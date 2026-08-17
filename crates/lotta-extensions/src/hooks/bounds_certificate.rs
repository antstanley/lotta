use super::test_support::*;
use std::sync::Arc;
#[test]
fn actual_63_then_64_and_65_preserves_snapshot() {
    let registry = HookRegistry::new();
    let o = owner("o");
    for count in [63, 64] {
        let hooks = (0..count)
            .map(|n| {
                HookRegistration::new(
                    o.clone(),
                    id(&format!("h{n}")),
                    HookEvent::Stop,
                    None,
                    command(),
                )
                .unwrap()
            })
            .collect();
        registry.replace_owner(&o, hooks).unwrap();
        assert_eq!(
            registry.snapshot().unwrap().event(HookEvent::Stop).len(),
            count
        );
    }
    let before = registry.snapshot().unwrap();
    let content: Vec<_> = before
        .event(HookEvent::Stop)
        .iter()
        .map(|h| h.hook_id.as_str().to_owned())
        .collect();
    let over = (0..65)
        .map(|n| {
            HookRegistration::new(
                o.clone(),
                id(&format!("x{n}")),
                HookEvent::Stop,
                None,
                command(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(
        registry.replace_owner(&o, over).unwrap_err(),
        HookLoadError::EventLimit
    );
    let after = registry.snapshot().unwrap();
    assert!(Arc::ptr_eq(&before, &after));
    assert_eq!(
        content,
        after
            .event(HookEvent::Stop)
            .iter()
            .map(|h| h.hook_id.as_str().to_owned())
            .collect::<Vec<_>>()
    );
}
#[test]
fn concurrent_two_owner_barrier_both_survive() {
    let registry = Arc::new(HookRegistry::new());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let threads = ["a", "b"].map(|name| {
        let registry = Arc::clone(&registry);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let o = owner(name);
            let h = HookRegistration::new(o.clone(), id(name), HookEvent::Stop, None, command())
                .unwrap();
            barrier.wait();
            registry.replace_owner(&o, vec![h]).unwrap();
        })
    });
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot.event(HookEvent::Stop).len(), 2);
}
#[test]
fn replace_and_remove_owner_touch_only_own_hooks() {
    let registry = HookRegistry::new();
    let a = owner("a");
    let b = owner("b");
    for (o, n) in [(&a, "a1"), (&b, "b1")] {
        registry
            .replace_owner(
                o,
                vec![
                    HookRegistration::new(o.clone(), id(n), HookEvent::Stop, None, command())
                        .unwrap(),
                ],
            )
            .unwrap();
    }
    registry
        .replace_owner(
            &a,
            vec![
                HookRegistration::new(a.clone(), id("a2"), HookEvent::Stop, None, command())
                    .unwrap(),
            ],
        )
        .unwrap();
    assert_eq!(
        registry
            .snapshot()
            .unwrap()
            .event(HookEvent::Stop)
            .iter()
            .map(|h| h.hook_id.as_str())
            .collect::<Vec<_>>(),
        ["b1", "a2"]
    );
    registry.remove_owner(&a).unwrap();
    assert_eq!(
        registry.snapshot().unwrap().event(HookEvent::Stop)[0].owner,
        b
    );
}
