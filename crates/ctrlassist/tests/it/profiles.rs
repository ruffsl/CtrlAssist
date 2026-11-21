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
    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| *e == StateEvent::Hydrated).await;
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
        .put_profile("couch", "preset = \"priority\"\n")
        .await
        .unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::InvalidProfile));
    assert!(refused.to_string().starts_with("preset: "), "{refused}");
    let refused = client.put_profile("Not A Name", RIGHT).await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::BadRequest));
    assert_eq!(client.profile("couch").unwrap().unwrap().toml, RIGHT);

    // A daemon that starts later still keeps it, from its first state on.
    daemon.stop().await;
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonDisappeared).await;
    assert!(client.list_profiles().unwrap().is_empty());
    let daemon = start_daemon(&socket).await;
    next_matching(&mut watcher, |e| *e == StateEvent::Hydrated).await;
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
