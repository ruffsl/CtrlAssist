//! Does what one leaf serves reach another leaf through the hub?
//!
//! The hub listens on a Unix socket, as the daemon does. The window is
//! a connect-only peer, as every CtrlAssist client is today. The tray
//! serves: a liveliness token, a queryable, and a cached publication.
//! Each case says what the window saw of the three, with the hub a peer
//! or a router and the tray on a connect-only peer link or a
//! client-mode one. Then the hub is restarted under both leaves.

use std::path::Path;
use std::time::Duration;

use zenoh::Config;
use zenoh::sample::SampleKind;
use zenoh_ext::{
    AdvancedPublisherBuilderExt, AdvancedSubscriberBuilderExt, CacheConfig, HistoryConfig,
    MissDetectionConfig,
};

const TOKEN: &str = "spike/alive/tray";
const RPC: &str = "spike/rpc/tray/info";
const STATE: &str = "spike/tray/settings";
/// How long the window is given to see something: generous, since the
/// question is whether, not how fast.
const WAIT: Duration = Duration::from_millis(1500);

fn set(config: &mut Config, key: &str, value: &str) {
    config.insert_json5(key, value).unwrap();
}

fn base() -> Config {
    let mut config = Config::default();
    set(&mut config, "scouting/multicast/enabled", "false");
    set(&mut config, "scouting/gossip/enabled", "false");
    config
}

fn endpoint(socket: &Path) -> String {
    format!("[\"unixsock-stream/{}\"]", socket.display())
}

/// The daemon's config, in `mode`.
fn hub(socket: &Path, mode: &str) -> Config {
    let mut config = base();
    set(&mut config, "mode", &format!("\"{mode}\""));
    set(&mut config, "listen/endpoints", &endpoint(socket));
    set(&mut config, "timestamping/enabled", "true");
    config
}

/// A long-running client's config today: a connect-only peer that dials
/// forever.
fn peer_leaf(socket: &Path) -> Config {
    let mut config = base();
    set(&mut config, "mode", "\"peer\"");
    set(&mut config, "listen/endpoints", "[]");
    set(&mut config, "connect/endpoints", &endpoint(socket));
    set(&mut config, "connect/exit_on_failure", "false");
    set(&mut config, "connect/timeout_ms", "-1");
    set(&mut config, "open/return_conditions/connect_scouted", "false");
    config
}

/// A client-mode link, which fails at open with no hub.
fn client_leaf(socket: &Path) -> Config {
    let mut config = base();
    set(&mut config, "mode", "\"client\"");
    set(&mut config, "connect/endpoints", &endpoint(socket));
    config
}

/// What the tray serves, held for as long as it serves it.
struct Serving {
    _token: zenoh::liveliness::LivelinessToken,
    _queryable: tokio::task::JoinHandle<()>,
    _publisher: zenoh_ext::AdvancedPublisher<'static>,
}

async fn serve(session: &zenoh::Session) -> Serving {
    let token = session.liveliness().declare_token(TOKEN).await.unwrap();
    let queryable = session.declare_queryable(RPC).await.unwrap();
    let answering = tokio::spawn(async move {
        while let Ok(query) = queryable.recv_async().await {
            let _ = query.reply(RPC, "0.5.0").await;
        }
    });
    let publisher = session
        .declare_publisher(STATE)
        .cache(CacheConfig::default().max_samples(1))
        // As the daemon publishes: numbered by a counter, so a leaf with
        // no timestamps of its own can keep a cache.
        .sample_miss_detection(MissDetectionConfig::default())
        .publisher_detection()
        .await
        .unwrap();
    publisher.put("held=true").await.unwrap();
    Serving {
        _token: token,
        _queryable: answering,
        _publisher: publisher,
    }
}

/// What the window sees of the tray: its token, a reply, its state.
async fn seen(window: &zenoh::Session) -> (bool, bool, bool) {
    let alive = window
        .liveliness()
        .declare_subscriber(TOKEN)
        .history(true)
        .await
        .unwrap();
    let token = matches!(
        tokio::time::timeout(WAIT, alive.recv_async()).await,
        Ok(Ok(sample)) if sample.kind() == SampleKind::Put
    );
    let replies = window.get(RPC).timeout(WAIT).await.unwrap();
    let reply = matches!(
        tokio::time::timeout(WAIT, replies.recv_async()).await,
        Ok(Ok(reply)) if reply.result().is_ok()
    );
    let state = window
        .declare_subscriber(STATE)
        .history(HistoryConfig::default().detect_late_publishers())
        .subscriber_detection()
        .await
        .unwrap();
    let published = matches!(
        tokio::time::timeout(WAIT, state.recv_async()).await,
        Ok(Ok(_))
    );
    (token, reply, published)
}

fn say(case: &str, (token, reply, state): (bool, bool, bool)) {
    let word = |seen: bool| if seen { "seen" } else { "NOT seen" };
    println!(
        "{case:<52} token {:<9} reply {:<9} state {}",
        word(token),
        word(reply),
        word(state)
    );
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let dir = tempfile::Builder::new().prefix("sr").tempdir_in("/tmp").unwrap();
    for hub_mode in ["peer", "router"] {
        for tray_link in ["peer", "client"] {
            let socket = dir.path().join(format!("{hub_mode}-{tray_link}.sock"));
            let hub_session = zenoh::open(hub(&socket, hub_mode)).await.unwrap();
            let window = zenoh::open(peer_leaf(&socket)).await.unwrap();
            let tray = zenoh::open(if tray_link == "peer" {
                peer_leaf(&socket)
            } else {
                client_leaf(&socket)
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            let serving = serve(&tray).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            say(
                &format!("hub {hub_mode}, tray on a {tray_link} link: the window"),
                seen(&window).await,
            );
            // The hub itself, which every case should show seeing all.
            say(
                &format!("hub {hub_mode}, tray on a {tray_link} link: the hub"),
                seen(&hub_session).await,
            );

            if hub_mode == "router" && tray_link == "client" {
                // The daemon restarts under both. Does the tray's link
                // come back by itself, and what it serves with it?
                hub_session.close().await.unwrap();
                tokio::time::sleep(Duration::from_millis(500)).await;
                let hub_again = zenoh::open(hub(&socket, hub_mode)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(2500)).await;
                say(
                    "  after the hub restarts, same tray session: the window",
                    seen(&window).await,
                );
                println!("  the tray's old session says it is closed: {}", tray.is_closed());
                // Opened again, as a supervised link would be.
                drop(serving);
                let _ = tray.close().await;
                let tray_again = zenoh::open(client_leaf(&socket)).await.unwrap();
                let _serving = serve(&tray_again).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                say(
                    "  after the tray opens a new session: the window",
                    seen(&window).await,
                );
                // And a client-mode open with no hub at all.
                hub_again.close().await.unwrap();
                tokio::time::sleep(Duration::from_millis(300)).await;
                let refused = zenoh::open(client_leaf(&socket)).await.is_err();
                println!("  a client-mode open with no hub fails at once: {refused}");
                continue;
            }
            drop(serving);
            let _ = tray.close().await;
            let _ = window.close().await;
            let _ = hub_session.close().await;
        }
    }
}
