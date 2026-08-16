use super::*;
use crate::permissions::matcher::PermissionError;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("lotta-shell-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}
fn rejected(command: &str, cwd: &Path) {
    assert_eq!(
        analyze_shell(command, cwd, &[cwd.to_path_buf()]).unwrap_err(),
        PermissionError::UnsafeShell,
        "{command}"
    );
}
fn accepted(command: &str, cwd: &Path) {
    analyze_shell(command, cwd, &[cwd.to_path_buf()]).unwrap();
}

#[test]
fn safe_data_shell_words() {
    let cwd = root();
    for command in [
        "printf sh",
        "echo /bin/bash",
        "printf -- '--git-dir=/outside'",
    ] {
        let analysis = analyze_shell(command, &cwd, std::slice::from_ref(&cwd)).unwrap();
        assert!(analysis.canonical_paths().is_empty(), "{command}");
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn direct_and_wrapped_shell_launchers() {
    let cwd = root();
    for command in [
        "sh",
        "A=1 sh",
        "env A=1 sh",
        "env -i sh",
        "env --ignore-environment sh",
        "env --unset X sh",
        "env --unset=X sh",
        "env -- sh",
        "command -p sh",
        "xargs -0 sh",
        "xargs --null sh",
        "xargs -n 1 sh",
    ] {
        rejected(command, &cwd);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wrapper_queries_and_safe_commands() {
    let cwd = root();
    accepted("command -v sh", &cwd);
    accepted("command -V bash", &cwd);
    accepted("env printf sh", &cwd);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn find_launcher_matrix() {
    let cwd = root();
    for action in ["-exec", "-execdir", "-ok", "-okdir"] {
        rejected(&format!(r"find . {action} sh {{}} \;"), &cwd);
    }
    for command in ["find . -print", "find . -name sh", "find . -fprint inside"] {
        let analysis = analyze_shell(command, &cwd, std::slice::from_ref(&cwd)).unwrap();
        if command.contains("-fprint") {
            assert!(!analysis.is_read_only());
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn rg_launcher_matrix() {
    let cwd = root();
    fs::write(cwd.join("file"), "x").unwrap();
    for command in [
        "rg --pre sh pattern file",
        "rg --pre=sh pattern file",
        "rg --hostname-bin sh pattern file",
        "rg --hostname-bin=sh pattern file",
        "rg --search-zip pattern file",
    ] {
        rejected(command, &cwd);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_and_wrapped_path_options() {
    use std::os::unix::fs::symlink;
    let cwd = root();
    let outside = cwd.parent().unwrap().join(format!(
        "lotta-shell-outside-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(cwd.join("inside")).unwrap();
    fs::write(cwd.join("inside/file"), "x").unwrap();
    fs::write(cwd.join("inside/options"), "x").unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("file"), "x").unwrap();
    symlink(outside.join("file"), cwd.join("link")).unwrap();
    symlink(&outside, cwd.join("dir-link")).unwrap();

    for command in [
        "cat link",
        "env cat link",
        "command cat link",
        "git -C dir-link status",
        "git -C /outside status",
        "git -Cdir-link status",
        "git --git-dir dir-link status",
        "git --git-dir=dir-link status",
        "git --work-tree dir-link status",
        "git --work-tree=dir-link status",
        "grep -f link pattern inside/file",
        "rg --ignore-file link pattern inside/file",
        "xargs -a link echo",
        "xargs --arg-file=link echo",
        "xargs -alink echo",
        "env -C dir-link printf ok",
        "env --chdir=dir-link printf ok",
    ] {
        rejected(command, &cwd);
    }
    for command in [
        "cat inside/file",
        "env cat inside/file",
        "command cat inside/file",
        "git -C inside status",
        "git -Cinside status",
        "git --git-dir inside status",
        "git --git-dir=inside status",
        "git --work-tree inside status",
        "git --work-tree=inside status",
        "grep -f inside/options pattern inside/file",
        "rg --ignore-file inside/options pattern inside/file",
        "xargs -a inside/options echo",
        "xargs --arg-file=inside/options echo",
        "xargs -ainside/options echo",
        "env -C inside printf ok",
        "env --chdir=inside printf ok",
    ] {
        accepted(command, &cwd);
    }
    fs::remove_dir_all(cwd).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn separated_unsafe_git_options_are_not_readonly() {
    let cwd = root();
    for command in [
        "git diff --ext-diff",
        "git diff --output copy",
        "git diff --output=copy",
        "git diff --textconv helper",
        "git diff --textconv=helper",
        "git log --exec helper",
        "git log --exec=helper",
    ] {
        let analysis = analyze_shell(command, &cwd, std::slice::from_ref(&cwd)).unwrap();
        assert!(!analysis.is_read_only(), "{command}");
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn missing_path_option_values_are_unsafe() {
    let cwd = root();
    for command in [
        "git -C",
        "git --git-dir",
        "grep -f",
        "rg --ignore-file",
        "head --files0-from",
        "find . -fprint",
        "env --chdir",
        "xargs --arg-file",
    ] {
        rejected(command, &cwd);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn traversal_and_redirection_rejected() {
    let cwd = root();
    for command in [r"cat ..\shadow", "cat ../shadow", "cat file > copy"] {
        rejected(command, &cwd);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn command_substitution_rejected() {
    let cwd = root();
    rejected("cat $(printf allowed.txt)", &cwd);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn quoted_literals_and_compound_segments_are_independent() {
    let cwd = root();
    fs::write(cwd.join("allowed.txt"), "x").unwrap();
    let safe = analyze_shell(
        "cat 'allowed.txt'; printf '<literal>'",
        &cwd,
        std::slice::from_ref(&cwd),
    )
    .unwrap();
    assert_eq!(safe.segments().len(), 2);
    rejected("cat allowed.txt; cat /etc/shadow", &cwd);
    fs::remove_dir_all(cwd).unwrap();
}
