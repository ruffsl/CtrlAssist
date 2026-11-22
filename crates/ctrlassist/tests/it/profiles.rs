//! Profiles through the SDK, against a live daemon: checked, kept across
//! a restart, replaced and deleted.

use ctrlassist::{DaemonClient, ErrorCode, StateEvent, StateWatcher, StateWatcherConfig};

use crate::client::{eventually, next_matching, socket, start_daemon};

const RIGHT: &str = include_str!("../../../ctrlassist-protocol/tests/it/fixtures/preset.toml");

fn changed(name: &str) -> StateEvent {
    StateEvent::ProfileChanged {
        profile: name.to_string(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_is_checked_kept_replaced_and_deleted() {
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| {
        *e == StateEvent::Hydrated(ctrlassist::Service::Daemon)
    })
    .await;
    assert!(client.list_profiles().unwrap().is_empty());
    assert_eq!(client.profile("couch").unwrap(), None);

    // Checking keeps nothing, and says where a profile is wrong.
    assert_eq!(client.validate_profile(RIGHT).await.unwrap(), None);
    let problem = client
        .validate_profile("[node.share]\ndemux = \"unicast\"\ninput = \"nobody\"\noutputs = 2\n")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(problem.at, "node.share.input");
    assert!(problem.to_string().starts_with("node.share.input: "));
    assert!(client.list_profiles().unwrap().is_empty());

    client.put_profile("couch", RIGHT).await.unwrap();
    next_matching(&mut watcher, |e| *e == changed("couch")).await;
    let kept = client.profile("couch").unwrap().unwrap();
    assert_eq!((kept.name.as_str(), kept.toml.as_str()), ("couch", RIGHT));
    assert_eq!(kept.problem, None);

    // A wrong one is refused with where and what, and the kept one stays.
    let refused = client
        .put_profile("couch", "mode = \"priority\"\n")
        .await
        .unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::InvalidProfile));
    assert!(refused.to_string().starts_with("mode: "), "{refused}");
    let refused = client.put_profile("Not A Name", RIGHT).await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::BadRequest));
    assert_eq!(client.profile("couch").unwrap().unwrap().toml, RIGHT);

    // A daemon that starts later still keeps it, from its first state on.
    daemon.stop().await;
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonDisappeared).await;
    assert!(client.list_profiles().unwrap().is_empty());
    let daemon = start_daemon(&socket).await;
    next_matching(&mut watcher, |e| {
        *e == StateEvent::Hydrated(ctrlassist::Service::Daemon)
    })
    .await;
    let names: Vec<String> = client
        .list_profiles()
        .unwrap()
        .into_iter()
        .map(|profile| profile.name)
        .collect();
    assert_eq!(names, ["couch"]);

    // Replaced, then deleted.
    let other = RIGHT.replace("priority", "average");
    client.put_profile("couch", &other).await.unwrap();
    eventually("the replacement never arrived", async || {
        client.profile("couch").unwrap().unwrap().toml == other
    })
    .await;
    client.delete_profile("couch").await.unwrap();
    eventually("the profile never went", async || {
        client.profile("couch").unwrap().is_none()
    })
    .await;
    let unknown = client.delete_profile("couch").await.unwrap_err();
    assert_eq!(unknown.rpc_code(), Some(ErrorCode::UnknownProfile));
    daemon.stop().await;
}

/// Files changed some other way than through the daemon are taken up by
/// a reload, which says what it found: new and changed profiles, ones
/// that are gone, and ones with something wrong, which are still listed.
#[tokio::test(flavor = "multi_thread")]
async fn a_reload_takes_up_profiles_written_by_hand() {
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| {
        *e == StateEvent::Hydrated(ctrlassist::Service::Daemon)
    })
    .await;
    client.put_profile("couch", RIGHT).await.unwrap();
    client.put_profile("desk", RIGHT).await.unwrap();
    eventually("the profiles never arrived", async || {
        client.list_profiles().unwrap().len() == 2
    })
    .await;
    // Nothing changed on disk: nothing to say.
    assert_eq!(client.reload().await.unwrap(), Default::default());

    let profiles = socket.with_file_name("config").join("profiles");
    let other = RIGHT.replace("priority", "average");
    std::fs::write(profiles.join("couch.toml"), &other).unwrap();
    std::fs::write(profiles.join("den.toml"), "mode = \"priority\"\n").unwrap();
    std::fs::write(profiles.join("Not A Name.toml"), RIGHT).unwrap();
    std::fs::remove_file(profiles.join("desk.toml")).unwrap();
    // Until asked, the daemon goes by what it read.
    assert_eq!(client.profile("couch").unwrap().unwrap().toml, RIGHT);

    let reloaded = client.reload().await.unwrap();
    assert_eq!(reloaded.changed, ["couch", "den"]);
    assert_eq!(reloaded.removed, ["desk"]);
    assert!(reloaded.updating.is_empty(), "nothing runs");
    assert_eq!(reloaded.wrong.len(), 1);
    assert_eq!(reloaded.wrong[0].name, "den");
    assert!(reloaded.wrong[0].problem.is_some());
    eventually("the reload's changes never arrived", async || {
        let names: Vec<String> = client
            .list_profiles()
            .unwrap()
            .into_iter()
            .map(|profile| profile.name)
            .collect();
        names == ["couch", "den"] && client.profile("couch").unwrap().unwrap().toml == other
    })
    .await;
    assert!(client.profile("den").unwrap().unwrap().problem.is_some());
    daemon.stop().await;
}

/// The daemon describes a preset's fields, and a profile written from
/// that description alone is one it keeps.
#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_serves_a_preset_s_fields_and_keeps_what_they_write() {
    use ctrlassist::form;
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    let fields = client.preset_form().await.unwrap();
    assert_eq!(fields, form::fields());
    assert_eq!(fields[0].key, form::PRESET);

    let mut values = form::Values::new();
    for (key, value) in [
        ("mode", "average"),
        ("primary", "0c:27:56:4f:bd:14"),
        ("assist", "model:054c:09cc"),
        ("rumble", "assist"),
        ("managed", "true"),
    ] {
        values.insert(key.to_string(), value.to_string());
    }
    let toml = form::to_toml(&fields, &values);
    client.put_profile("couch", &toml).await.unwrap();
    eventually("the profile never arrived", async || {
        client.profile("couch").unwrap().is_some()
    })
    .await;
    let kept = client.profile("couch").unwrap().unwrap();
    assert_eq!(kept.problem, None);
    let read = form::from_toml(&fields, &kept.toml).unwrap();
    assert_eq!(read["rumble"], "assist");
    assert_eq!(read["identity"], "generic");
    daemon.stop().await;
}
