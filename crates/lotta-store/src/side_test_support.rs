use crate::atomic::AtomicObserver;
use crate::side::{
    ChannelFile, ProjectFile, SidePaths, channels as channel_store, crons,
    project as project_store, settings,
};
use crate::{StoreError, StoreErrorKind};
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::roots::TemporaryRoot;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

struct Harness {
    root: TemporaryRoot,
    paths: SidePaths,
    workspace: PathBuf,
}

impl Harness {
    fn new(label: &str) -> Self {
        let root = TemporaryRoot::new(label).expect("temporary root");
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        let paths = SidePaths::new(&home, None, [workspace.clone()]).expect("paths");
        assert_eq!(
            paths.settings().expect("settings"),
            home.join(".letta/settings.json")
        );
        assert_eq!(
            paths.crons().expect("crons"),
            home.join(".letta/crons.json")
        );
        assert_eq!(
            paths.run_log("schedule-fixture").expect("run"),
            home.join(".letta/runs/schedule-fixture.jsonl")
        );
        assert_eq!(
            paths.channels_root().expect("channels"),
            home.join(".letta/channels")
        );
        assert_exact_targets(&paths, &home, &workspace);
        copy_fixtures(&paths, &workspace);
        Self {
            root,
            paths,
            workspace,
        }
    }
}

fn assert_exact_targets(paths: &SidePaths, home: &Path, workspace: &Path) {
    assert_eq!(
        paths.pending_control().expect("pending"),
        home.join(".letta/channels/pending-control-requests.json")
    );
    let channel = home.join(".letta/channels/telegram");
    for (file, name) in [
        (ChannelFile::Config, "config.yaml"),
        (ChannelFile::Accounts, "accounts.json"),
        (ChannelFile::Routing, "routing.yaml"),
        (ChannelFile::Pairing, "pairing.yaml"),
        (ChannelFile::Targets, "targets.json"),
    ] {
        assert_eq!(
            paths.channel_file("telegram", file).expect("channel file"),
            channel.join(name)
        );
    }
    assert_eq!(
        paths
            .project_file(workspace, ProjectFile::Settings)
            .expect("project settings"),
        workspace.join(".letta/settings.json")
    );
    assert_eq!(
        paths
            .project_file(workspace, ProjectFile::LocalSettings)
            .expect("project local settings"),
        workspace.join(".letta/settings.local.json")
    );
}

fn fixture(path: &str) -> Vec<u8> {
    FixtureLoader::new()
        .load_bytes(format!("persistence/side_stores/{path}"))
        .expect("fixture")
}

fn seed(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parents");
    std::fs::write(path, bytes).expect("seed fixture");
}

fn copy_fixtures(paths: &SidePaths, workspace: &Path) {
    let pairs = [
        (
            paths.settings().expect("settings"),
            "home/.letta/settings.json",
        ),
        (paths.crons().expect("crons"), "letta_home/crons.json"),
        (
            paths.run_log("schedule-fixture").expect("run"),
            "letta_home/runs/schedule-fixture.jsonl",
        ),
        (
            paths.pending_control().expect("pending"),
            "home/.letta/channels/pending-control-requests.json",
        ),
    ];
    for (target, source) in pairs {
        seed(&target, &fixture(source));
    }
    for file in channel_files() {
        let name = paths
            .channel_file("telegram", file)
            .expect("channel")
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        seed(
            &paths.channel_file("telegram", file).expect("channel"),
            &fixture(&format!("home/.letta/channels/telegram/{name}")),
        );
    }
    for file in [ProjectFile::Settings, ProjectFile::LocalSettings] {
        let target = paths.project_file(workspace, file).expect("project");
        let name = target.file_name().expect("name").to_string_lossy();
        seed(&target, &fixture(&format!("workspace/.letta/{name}")));
    }
}

fn channel_files() -> [ChannelFile; 5] {
    [
        ChannelFile::Config,
        ChannelFile::Accounts,
        ChannelFile::Routing,
        ChannelFile::Pairing,
        ChannelFile::Targets,
    ]
}

pub(crate) mod paths {
    use super::*;

    pub(crate) fn settings() {
        path_case(&PathCase::Settings);
    }
    pub(crate) fn crons() {
        path_case(&PathCase::Crons);
    }
    pub(crate) fn run() {
        path_case(&PathCase::Run);
    }
    pub(crate) fn pending() {
        path_case(&PathCase::Pending);
    }
    pub(crate) fn config() {
        path_case(&PathCase::Channel(ChannelFile::Config));
    }
    pub(crate) fn accounts() {
        path_case(&PathCase::Channel(ChannelFile::Accounts));
    }
    pub(crate) fn routing() {
        path_case(&PathCase::Channel(ChannelFile::Routing));
    }
    pub(crate) fn pairing() {
        path_case(&PathCase::Channel(ChannelFile::Pairing));
    }
    pub(crate) fn targets() {
        path_case(&PathCase::Channel(ChannelFile::Targets));
    }
    pub(crate) fn project_settings() {
        path_case(&PathCase::Project(ProjectFile::Settings));
    }
    pub(crate) fn project_local() {
        path_case(&PathCase::Project(ProjectFile::LocalSettings));
    }
    pub(crate) fn letta_home_override() {
        let root = TemporaryRoot::new("side-path-override").expect("root");
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        let override_root = root.path().join("letta_home");
        let paths =
            SidePaths::new(&home, Some(override_root.clone()), [workspace.clone()]).expect("paths");
        assert_eq!(
            paths.crons().expect("crons"),
            override_root.join("crons.json")
        );
        assert_eq!(
            paths.run_log("schedule-fixture").expect("run"),
            override_root.join("runs/schedule-fixture.jsonl")
        );
        assert_eq!(
            paths.settings().expect("settings"),
            home.join(".letta/settings.json")
        );
        assert_eq!(
            paths.channels_root().expect("channels"),
            home.join(".letta/channels")
        );
        copy_fixtures(&paths, &workspace);
        let harness = Harness {
            root,
            paths,
            workspace,
        };
        path_case_with(&harness, &PathCase::Crons);
        path_case_with(&harness, &PathCase::Run);
        assert!(!home.join(".letta/crons.json").exists());
        assert!(!home.join(".letta/runs/schedule-fixture.jsonl").exists());
    }

    enum PathCase {
        Settings,
        Crons,
        Run,
        Pending,
        Channel(ChannelFile),
        Project(ProjectFile),
    }

    fn path_case(case: &PathCase) {
        path_case_with(&Harness::new("side-path-roundtrip"), case);
    }

    fn path_case_with(harness: &Harness, case: &PathCase) {
        let before = snapshot(harness);
        let alternate = b"alternate opaque bytes\n";
        write_case(harness, case, alternate);
        assert_eq!(read_case(harness, case), alternate);
        let after = snapshot(harness);
        assert_eq!(changed(&before, &after), vec![case_path(harness, case)]);
    }

    fn write_case(harness: &Harness, case: &PathCase, bytes: &[u8]) {
        match case {
            PathCase::Settings => settings::write(&harness.paths, bytes),
            PathCase::Crons => crons::write(&harness.paths, bytes),
            PathCase::Run => crons::write_run(&harness.paths, "schedule-fixture", bytes),
            PathCase::Pending => channel_store::write_pending(&harness.paths, bytes),
            PathCase::Channel(file) => {
                channel_store::write(&harness.paths, "telegram", *file, bytes)
            }
            PathCase::Project(file) => {
                project_store::write(&harness.paths, &harness.workspace, *file, bytes)
            }
        }
        .expect("public write");
    }

    fn read_case(harness: &Harness, case: &PathCase) -> Vec<u8> {
        match case {
            PathCase::Settings => settings::read(&harness.paths),
            PathCase::Crons => crons::read(&harness.paths),
            PathCase::Run => crons::read_run(&harness.paths, "schedule-fixture"),
            PathCase::Pending => channel_store::read_pending(&harness.paths),
            PathCase::Channel(file) => channel_store::read(&harness.paths, "telegram", *file),
            PathCase::Project(file) => {
                project_store::read(&harness.paths, &harness.workspace, *file)
            }
        }
        .expect("public read")
        .bytes()
        .to_vec()
    }

    fn case_path(harness: &Harness, case: &PathCase) -> PathBuf {
        match case {
            PathCase::Settings => harness.paths.settings(),
            PathCase::Crons => harness.paths.crons(),
            PathCase::Run => harness.paths.run_log("schedule-fixture"),
            PathCase::Pending => harness.paths.pending_control(),
            PathCase::Channel(file) => harness.paths.channel_file("telegram", *file),
            PathCase::Project(file) => harness.paths.project_file(&harness.workspace, *file),
        }
        .expect("path")
    }

    fn snapshot(harness: &Harness) -> Vec<(PathBuf, Vec<u8>)> {
        let mut output = Vec::new();
        let cases = [
            PathCase::Settings,
            PathCase::Crons,
            PathCase::Run,
            PathCase::Pending,
            PathCase::Channel(ChannelFile::Config),
            PathCase::Channel(ChannelFile::Accounts),
            PathCase::Channel(ChannelFile::Routing),
            PathCase::Channel(ChannelFile::Pairing),
            PathCase::Channel(ChannelFile::Targets),
            PathCase::Project(ProjectFile::Settings),
            PathCase::Project(ProjectFile::LocalSettings),
        ];
        for case in cases {
            output.push((case_path(harness, &case), read_case(harness, &case)));
        }
        output
    }

    fn changed(before: &[(PathBuf, Vec<u8>)], after: &[(PathBuf, Vec<u8>)]) -> Vec<PathBuf> {
        before
            .iter()
            .zip(after)
            .filter(|((_, left), (_, right))| left != right)
            .map(|((path, _), _)| path.clone())
            .collect()
    }
}

pub(crate) mod channels {
    use super::*;

    pub(crate) fn preserves_plugin_files() {
        let harness = Harness::new("side-plugin-preserve");
        let channel = harness.paths.channel_dir("telegram").expect("channel");
        let plugin = channel.join("plugin-state.bin");
        let nested = channel.join("plugin/state.bin");
        let plugin_bytes = b"\0opaque plugin bytes\n";
        let nested_bytes = b"nested opaque bytes\0\n";
        seed(&plugin, plugin_bytes);
        seed(&nested, nested_bytes);
        let before = std::fs::metadata(&plugin).expect("metadata");
        let nested_before = std::fs::metadata(&nested).expect("metadata");
        let listing = channel_store::list(&harness.paths, "telegram").expect("listing");
        channel_store::write(
            &harness.paths,
            "telegram",
            ChannelFile::Accounts,
            b"changed accounts\n",
        )
        .expect("public rewrite");
        let after = std::fs::metadata(&plugin).expect("metadata");
        let nested_after = std::fs::metadata(&nested).expect("metadata");
        assert_eq!(std::fs::read(&plugin).expect("plugin"), plugin_bytes);
        assert_eq!(std::fs::read(&nested).expect("nested"), nested_bytes);
        assert_eq!(
            before.modified().expect("mtime"),
            after.modified().expect("mtime")
        );
        assert_eq!(
            nested_before.modified().expect("mtime"),
            nested_after.modified().expect("mtime")
        );
        #[cfg(unix)]
        {
            assert_eq!(inode(&before), inode(&after));
            assert_eq!(inode(&nested_before), inode(&nested_after));
        }
        let after_listing = channel_store::list(&harness.paths, "telegram").expect("listing");
        assert_eq!(listing, after_listing);
        assert!(after_listing.contains(&PathBuf::from("plugin-state.bin")));
        assert!(after_listing.contains(&PathBuf::from("plugin/state.bin")));
    }

    #[cfg(unix)]
    fn inode(metadata: &std::fs::Metadata) -> u64 {
        use std::os::unix::fs::MetadataExt as _;
        metadata.ino()
    }
}

pub(crate) mod project {
    use super::*;

    pub(crate) fn scope_gate() {
        let harness = Harness::new("side-project-scope");
        let outside = harness.root.path().join("outside");
        let target = outside.join(".letta/settings.json");
        seed(&target, b"outside secret");
        let observer = AccessObserver::default();
        let error = project_store::read_observed(
            &harness.paths,
            &outside,
            ProjectFile::Settings,
            &observer,
        )
        .expect_err("out of scope");
        assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
        assert_eq!(observer.calls(), 0);
        assert_eq!(
            project_store::read_observed(
                &harness.paths,
                &harness.workspace,
                ProjectFile::Settings,
                &observer,
            )
            .expect("settings")
            .bytes(),
            fixture("workspace/.letta/settings.json")
        );
        assert_eq!(
            project_store::read_observed(
                &harness.paths,
                &harness.workspace,
                ProjectFile::LocalSettings,
                &observer,
            )
            .expect("local")
            .bytes(),
            fixture("workspace/.letta/settings.local.json")
        );
        assert_eq!(observer.calls(), 2);
    }

    #[derive(Default)]
    struct AccessObserver(std::sync::atomic::AtomicUsize);
    impl AccessObserver {
        fn calls(&self) -> usize {
            self.0.load(std::sync::atomic::Ordering::Relaxed)
        }
    }
    impl crate::side::io::SideReadObserver for AccessObserver {
        fn before_access(&self, _path: &Path) -> Result<(), StoreError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    }
}

type Writer = fn(&Harness, &[u8], &dyn AtomicObserver) -> Result<(), StoreError>;

pub(crate) fn conflict_body() {
    let families: [Writer; 5] = [
        |h, b, o| settings::write_observed(&h.paths, b, o),
        |h, b, o| crons::write_observed(&h.paths, b, o),
        |h, b, o| crons::write_run_observed(&h.paths, "schedule-fixture", b, o),
        |h, b, o| channel_store::write_observed(&h.paths, "telegram", ChannelFile::Accounts, b, o),
        |h, b, o| {
            project_store::write_observed(&h.paths, &h.workspace, ProjectFile::Settings, b, o)
        },
    ];
    for writer in families {
        let harness = Harness::new("side-conflict");
        let observer = Replacer;
        let error = writer(&harness, b"candidate", &observer).expect_err("conflict");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(
            std::fs::read(error.path()).expect("external survives"),
            b"external!"
        );
        assert!(no_temps(error.path().parent().expect("parent")));
    }
}

pub(crate) fn read_mutation_conflict_body() {
    let harness = Harness::new("side-read-conflict");
    let target = harness.paths.settings().expect("settings");
    let observer = ReadReplacer;
    let error = crate::side::io::read_observed(&target, &observer).expect_err("conflict");
    assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
    assert!(
        std::fs::read(&target)
            .expect("replacement")
            .iter()
            .all(|byte| *byte == b'x')
    );
}

struct ReadReplacer;
impl crate::side::io::SideReadObserver for ReadReplacer {
    fn after_first_revision(&self, target: &Path) -> Result<(), StoreError> {
        let length = std::fs::metadata(target)
            .map_err(|error| StoreError::from_io(target, &error))?
            .len();
        let length =
            usize::try_from(length).map_err(|_| StoreError::new(StoreErrorKind::Limit, target))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, target))?;
        bytes.resize(length, b'x');
        std::fs::write(target, bytes).map_err(|error| StoreError::from_io(target, &error))
    }
}

struct Replacer;
impl AtomicObserver for Replacer {
    fn before_rename(&self, target: &Path) -> Result<(), StoreError> {
        std::fs::write(target, b"external!").map_err(|error| StoreError::from_io(target, &error))
    }
}

fn no_temps(parent: &Path) -> bool {
    std::fs::read_dir(parent).expect("entries").all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".lotta-write-")
    })
}

pub(crate) fn negatives_body() {
    let harness = Harness::new("side-negatives");
    assert!(SidePaths::new("relative", None, []).is_err());
    assert!(SidePaths::new(harness.paths.home(), None, [PathBuf::from("relative")]).is_err());
    for id in ["", ".", "..", "a/b", "a\\b", "/absolute", "a\0b"] {
        assert!(harness.paths.run_log(id).is_err());
        assert!(harness.paths.channel_dir(id).is_err());
    }
    let missing = SidePaths::new(harness.root.path().join("missing"), None, []).expect("paths");
    assert_eq!(
        settings::read(&missing).expect_err("missing").kind(),
        StoreErrorKind::NotFound
    );
    let source = settings::read(&harness.paths).expect("revision");
    std::fs::write(harness.paths.settings().expect("path"), b"external").expect("mutate");
    let error = settings::write_expected(&harness.paths, &source, b"candidate").expect_err("stale");
    assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
}

pub(crate) fn limits_body() {
    path_byte_limits();
    path_depth_limits();
    id_byte_limits();
    workspace_limits();
    payload_limits();
    channel_file_limits();
    channel_depth_limits();
    channel_entry_limits();
}

#[cfg(unix)]
pub(crate) fn unix_confinement_body() {
    unix_non_utf8_rejected();
    unix_symlinks_rejected();
    unix_special_file_rejected();
}

#[cfg(unix)]
fn lexical_root(bytes: usize, depth: usize) -> PathBuf {
    assert!((2..=65).contains(&depth));
    assert!(bytes >= depth * 2 - 1);
    let payload = bytes - (depth - 1);
    let base = payload / (depth - 1);
    let extra = payload % (depth - 1);
    let mut path = PathBuf::from("/");
    for index in 0..depth - 1 {
        path.push("x".repeat(base + usize::from(index < extra)));
    }
    assert_eq!(path.to_str().expect("utf8").len(), bytes);
    assert_eq!(path.components().count(), depth);
    path
}

fn path_byte_limits() {
    #[cfg(unix)]
    {
        for bytes in [4_095, 4_096] {
            let root = lexical_root(bytes, 64);
            assert!(SidePaths::new(&root, Some(root.clone()), []).is_ok());
        }
        let root = lexical_root(4_097, 64);
        assert_eq!(
            SidePaths::new(&root, Some(root.clone()), [])
                .expect_err("path bytes above")
                .kind(),
            StoreErrorKind::InvalidPath
        );
    }
}

fn path_depth_limits() {
    #[cfg(unix)]
    {
        for depth in [63, 64] {
            let root = lexical_root(depth * 2 - 1, depth);
            assert!(SidePaths::new(&root, Some(root.clone()), []).is_ok());
        }
        let root = lexical_root(129, 65);
        assert_eq!(
            SidePaths::new(&root, Some(root.clone()), [])
                .expect_err("path depth above")
                .kind(),
            StoreErrorKind::InvalidPath
        );
    }
}

fn id_byte_limits() {
    let harness = Harness::new("side-id-limits");
    for bytes in [254, 255] {
        let id = "i".repeat(bytes);
        assert!(harness.paths.run_log(&id).is_ok());
        assert!(harness.paths.channel_dir(&id).is_ok());
    }
    let id = "i".repeat(256);
    assert_eq!(
        harness.paths.run_log(&id).expect_err("run id above").kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(
        harness
            .paths
            .channel_dir(&id)
            .expect_err("channel id above")
            .kind(),
        StoreErrorKind::InvalidPath
    );
}

fn workspace_limits() {
    let root = TemporaryRoot::new("side-workspace-limits").expect("root");
    let home = root.path().join("home");
    let workspaces = |count: usize| {
        (0..count)
            .map(|index| root.path().join(format!("workspace-{index:03}")))
            .collect::<Vec<_>>()
    };
    for count in [255, 256] {
        assert!(SidePaths::new(&home, None, workspaces(count)).is_ok());
    }
    assert_eq!(
        SidePaths::new(&home, None, workspaces(257))
            .expect_err("workspaces above")
            .kind(),
        StoreErrorKind::Limit
    );
}

fn filled_bytes(length: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).expect("bounded allocation");
    bytes.resize(length, b'p');
    if let Some(last) = bytes.last_mut() {
        *last = b'\n';
    }
    bytes
}

fn payload_limits() {
    let harness = Harness::new("side-payload-limits");
    let target = harness.paths.settings().expect("settings");
    for length in [
        crate::side::io::SIDE_FILE_BYTES_MAX - 1,
        crate::side::io::SIDE_FILE_BYTES_MAX,
    ] {
        let bytes = filled_bytes(length);
        let expected_hash: [u8; 32] = Sha256::digest(&bytes).into();
        settings::write(&harness.paths, &bytes).expect("bounded write");
        let actual = settings::read(&harness.paths).expect("bounded read");
        assert_eq!(actual.bytes().len(), length);
        assert_eq!(actual.bytes().last(), Some(&b'\n'));
        let actual_hash: [u8; 32] = Sha256::digest(actual.bytes()).into();
        assert_eq!(actual_hash, expected_hash);
        assert!(no_temps(target.parent().expect("parent")));
    }
    let before = std::fs::read(&target).expect("snapshot");
    let above = filled_bytes(crate::side::io::SIDE_FILE_BYTES_MAX + 1);
    assert_eq!(
        settings::write(&harness.paths, &above)
            .expect_err("payload above")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(std::fs::read(&target).expect("unchanged"), before);
    assert!(no_temps(target.parent().expect("parent")));
}

fn empty_channel(label: &str) -> (TemporaryRoot, SidePaths, PathBuf) {
    let root = TemporaryRoot::new(label).expect("root");
    let home = root.path().join("home");
    let paths = SidePaths::new(&home, None, []).expect("paths");
    let channel = paths.channel_dir("plugin").expect("channel");
    std::fs::create_dir_all(&channel).expect("channel directory");
    (root, paths, channel)
}

fn channel_file_limits() {
    for count in [255, 256] {
        let (_root, paths, channel) = empty_channel("side-channel-file-limit");
        for index in 0..count {
            seed(&channel.join(format!("file-{index:03}.bin")), b"x");
        }
        let listing = channel_store::list(&paths, "plugin").expect("listing");
        assert_eq!(listing.len(), count);
        assert_eq!(listing.first(), Some(&PathBuf::from("file-000.bin")));
        assert_eq!(
            listing.last(),
            Some(&PathBuf::from(format!("file-{:03}.bin", count - 1)))
        );
    }
    let (_root, paths, channel) = empty_channel("side-channel-file-above");
    let sentinel = channel.join("sentinel.bin");
    seed(&sentinel, b"sentinel");
    for index in 0..256 {
        seed(&channel.join(format!("plugin-{index:03}.bin")), b"x");
    }
    let before = tree_snapshot(&channel);
    assert_eq!(
        channel_store::list(&paths, "plugin")
            .expect_err("files above")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(tree_snapshot(&channel), before);
    assert_eq!(std::fs::read(&sentinel).expect("sentinel"), b"sentinel");
    assert!(no_temps(&channel));
}

fn nested_file(channel: &Path, depth: usize) -> PathBuf {
    assert!(depth >= 1);
    let mut relative = PathBuf::new();
    for index in 0..depth - 1 {
        relative.push(format!("d{index}"));
    }
    relative.push("payload.bin");
    seed(&channel.join(&relative), b"payload");
    relative
}

fn channel_depth_limits() {
    for depth in [7, 8] {
        let (_root, paths, channel) = empty_channel("side-channel-depth-limit");
        let relative = nested_file(&channel, depth);
        assert_eq!(
            channel_store::list(&paths, "plugin").expect("depth listing"),
            vec![relative]
        );
    }
    let (_root, paths, channel) = empty_channel("side-channel-depth-above");
    let relative = nested_file(&channel, 9);
    let target = channel.join(relative);
    let before = tree_snapshot(&channel);
    assert_eq!(
        channel_store::list(&paths, "plugin")
            .expect_err("depth above")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(tree_snapshot(&channel), before);
    assert_eq!(std::fs::read(target).expect("sentinel"), b"payload");
    assert!(no_temps(&channel));
}

fn populate_entries(channel: &Path, count: usize) {
    let directory_count = count - 256;
    for index in 0..directory_count {
        std::fs::create_dir(channel.join(format!("dir-{index:03}"))).expect("directory");
    }
    for index in 0..256 {
        seed(&channel.join(format!("file-{index:03}.bin")), b"x");
    }
}

fn channel_entry_limits() {
    for entries in [511, 512] {
        let (_root, paths, channel) = empty_channel("side-channel-entry-limit");
        populate_entries(&channel, entries);
        assert_eq!(
            channel_store::list(&paths, "plugin")
                .expect("entry listing")
                .len(),
            256
        );
    }
    let (_root, paths, channel) = empty_channel("side-channel-entry-above");
    populate_entries(&channel, 513);
    let sentinel = channel.join("file-000.bin");
    let before = tree_snapshot(&channel);
    assert_eq!(
        channel_store::list(&paths, "plugin")
            .expect_err("entries above")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(tree_snapshot(&channel), before);
    assert_eq!(std::fs::read(&sentinel).expect("sentinel"), b"x");
    assert!(no_temps(&channel));
}

fn tree_snapshot(root: &Path) -> Vec<(PathBuf, bool, Vec<u8>)> {
    let mut queue = vec![PathBuf::new()];
    let mut snapshot = Vec::new();
    while let Some(relative) = queue.pop() {
        let directory = root.join(&relative);
        for entry in std::fs::read_dir(directory).expect("tree") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let child = relative.join(entry.file_name());
            if child.file_name().and_then(|name| name.to_str()) == Some(".lotta-storage.lock") {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                queue.push(child.clone());
                snapshot.push((child, true, Vec::new()));
            } else if metadata.is_file() {
                snapshot.push((child, false, std::fs::read(path).expect("file")));
            } else {
                snapshot.push((child, false, Vec::new()));
            }
        }
    }
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

#[cfg(unix)]
fn unix_non_utf8_rejected() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let root = TemporaryRoot::new("side-non-utf8").expect("root");
    let bad_root = PathBuf::from("/").join(OsString::from_vec(vec![0xff]));
    assert_eq!(
        SidePaths::new(&bad_root, None, [])
            .expect_err("non-utf8 root")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    let paths = SidePaths::new(root.path().join("home"), None, []).expect("paths");
    let channel = paths.channel_dir("plugin").expect("channel");
    std::fs::create_dir_all(&channel).expect("channel");
    let entry = channel.join(OsString::from_vec(vec![0xfe]));
    match std::fs::File::create(&entry) {
        Ok(_) => {
            let before = tree_snapshot(&channel);
            assert_eq!(
                channel_store::list(&paths, "plugin")
                    .expect_err("non-utf8 entry")
                    .kind(),
                StoreErrorKind::InvalidPath
            );
            assert_eq!(tree_snapshot(&channel), before);
            assert!(no_temps(&channel));
        }
        Err(error) if error.raw_os_error() == Some(92) => {
            // APFS rejects non-UTF-8 names before the channel API can observe them.
        }
        Err(error) => panic!("non-utf8 entry: {error}"),
    }
}

#[cfg(unix)]
fn unix_symlinks_rejected() {
    use std::os::unix::fs::symlink;

    let root = TemporaryRoot::new("side-symlinks").expect("root");
    let outside = root.path().join("outside.txt");
    seed(&outside, b"outside sentinel");
    let outside_before = std::fs::read(&outside).expect("outside");

    let home = root.path().join("home-known");
    let paths = SidePaths::new(&home, None, []).expect("paths");
    let target = paths.settings().expect("settings");
    std::fs::create_dir_all(target.parent().expect("parent")).expect("parent");
    symlink(&outside, &target).expect("known symlink");
    let tree_before = tree_snapshot(&home);
    assert_eq!(
        settings::read(&paths)
            .expect_err("known read symlink")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(
        settings::write(&paths, b"candidate")
            .expect_err("known write symlink")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(tree_snapshot(&home), tree_before);

    let parent_home = root.path().join("home-parent");
    let parent_paths = SidePaths::new(&parent_home, None, []).expect("paths");
    std::fs::create_dir_all(&parent_home).expect("home");
    symlink(
        root.path().join("outside-parent"),
        parent_home.join(".letta"),
    )
    .expect("parent symlink");
    let parent_before = tree_snapshot(&parent_home);
    assert_eq!(
        settings::write(&parent_paths, b"candidate")
            .expect_err("parent symlink")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(tree_snapshot(&parent_home), parent_before);

    let channel_home = root.path().join("home-channel");
    let channel_paths = SidePaths::new(&channel_home, None, []).expect("paths");
    let channel = channel_paths.channel_dir("plugin").expect("channel");
    std::fs::create_dir_all(&channel).expect("channel");
    let outside_tree = root.path().join("outside-tree");
    std::fs::create_dir_all(&outside_tree).expect("outside tree");
    seed(&outside_tree.join("sentinel.bin"), b"tree sentinel");
    symlink(&outside_tree, channel.join("nested")).expect("nested symlink");
    let channel_before = tree_snapshot(&channel);
    assert_eq!(
        channel_store::list(&channel_paths, "plugin")
            .expect_err("nested symlink")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(tree_snapshot(&channel), channel_before);
    assert_eq!(std::fs::read(&outside).expect("outside"), outside_before);
    assert_eq!(
        std::fs::read(outside_tree.join("sentinel.bin")).expect("outside tree"),
        b"tree sentinel"
    );
    assert!(no_temps(target.parent().expect("parent")));
    assert!(no_temps(&parent_home));
    assert!(no_temps(&channel));
}

#[cfg(unix)]
fn unix_special_file_rejected() {
    use std::os::unix::net::UnixListener;

    let root = PathBuf::from(format!("/tmp/lotta-side-socket-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = SidePaths::new(root.join("home"), None, []).expect("paths");
    let channel = paths.channel_dir("plugin").expect("channel");
    std::fs::create_dir_all(&channel).expect("channel");
    let socket = channel.join("plugin.sock");
    let listener = UnixListener::bind(&socket).expect("socket");
    let before = tree_snapshot(&channel);
    assert_eq!(
        channel_store::list(&paths, "plugin")
            .expect_err("special file")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(tree_snapshot(&channel), before);
    assert!(no_temps(&channel));
    drop(listener);
    std::fs::remove_dir_all(root).expect("cleanup");
}
