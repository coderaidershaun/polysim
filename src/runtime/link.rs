//! Link setup: bind the socket, hash the identity, resolve the peer topic names a config file gave
//! against the ones this strategy declares in code.

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tokio::runtime::Runtime;

use crate::config::{Config, ConfigError, PeerSubscription, RunIdentity};
use crate::hot::strategy::Strategy;
use crate::info;
use crate::link::{
    FrameGuard, LINK_NAME_LEN, LinkHash, LinkIdentity, PeerFeed, TopicId, TopicSet,
    schema_hash_of_fields,
};
use crate::registry::Registry;
use crate::time::boot_stamp_us;

use super::EngineError;

pub(super) struct BoundLink {
    pub socket: UdpSocket,
    pub identity: LinkIdentity,
    pub guard: FrameGuard,
    pub peers: Vec<PeerFeed>,
    pub topic_count: usize,
    pub feature_count: u16,
}

fn resolve_peers(
    subscriptions: &[PeerSubscription],
    strategy_topics: &[&'static str],
) -> Result<Vec<PeerFeed>, ConfigError> {
    subscriptions
        .iter()
        .map(|peer| {
            Ok(PeerFeed {
                address: peer.address,
                topics: resolve_topics(peer, strategy_topics)?,
            })
        })
        .collect()
}

fn resolve_topics(
    peer: &PeerSubscription,
    strategy_topics: &[&'static str],
) -> Result<TopicSet, ConfigError> {
    if peer.topics.is_empty() {
        return Ok(TopicSet::ALL);
    }
    let mut ids = Vec::with_capacity(peer.topics.len() + 1);
    for name in &peer.topics {
        let id =
            resolve_topic(name, strategy_topics).ok_or_else(|| ConfigError::UnknownLinkTopic {
                address: address_label(peer.address),
                topic: name.clone(),
            })?;
        ids.push(id);
    }
    // Lifecycle not opt-out (heartbeat; receiver can't tell dead feed from quiet).
    if !ids.contains(&TopicId::LIFECYCLE) {
        ids.push(TopicId::LIFECYCLE);
    }
    TopicSet::new(&ids).map_err(|_| ConfigError::TooManyPeerTopics {
        address: address_label(peer.address),
        count: ids.len(),
        max: crate::link::LINK_MAX_TOPICS,
    })
}

fn resolve_topic(name: &str, strategy_topics: &[&'static str]) -> Option<TopicId> {
    match name {
        "books" => Some(TopicId::BOOKS),
        "events" => Some(TopicId::EVENTS),
        "catalog_instruments" => Some(TopicId::CATALOG_INSTRUMENTS),
        "catalog_features" => Some(TopicId::CATALOG_FEATURES),
        "lifecycle" => Some(TopicId::LIFECYCLE),
        _ => strategy_topics
            .iter()
            .position(|declared| *declared == name)
            .map(TopicId::strategy),
    }
}

fn address_label(address: SocketAddr) -> Box<str> {
    address.to_string().into_boxed_str()
}

/// Bind socket + settle identity, guard and peer set, or Ok(None) with no link: block.
///
/// # Errors
/// [`EngineError::LinkNameTooLong`] for a name the catalog frame cannot carry,
/// [`EngineError::LinkBind`] for a taken or unavailable address, and
/// [`ConfigError`] via [`EngineError::Config`] for an unresolvable or over-long peer topic list.
///
/// [`ConfigError`]: crate::config::ConfigError
pub(super) fn link_bring_up<P>(
    config: &Config<P>,
    identity: &RunIdentity,
    strategy: &dyn Strategy,
    registry: &Registry,
    runtime: &Runtime,
) -> Result<Option<BoundLink>, EngineError> {
    let Some(link_config) = &config.link else {
        return Ok(None);
    };
    let feature_count = u16::try_from(strategy.features().len())
        .expect("strategy declares more than 65536 features — feature id overflow");
    // Feature names + displays in catalog frames; over-length caught here not mid-run.
    let wire_names = strategy
        .features()
        .iter()
        .map(|name| Box::<str>::from(*name))
        .chain(registry.instruments().iter().map(|row| row.display.clone()));
    for name in wire_names {
        if name.len() > LINK_NAME_LEN {
            return Err(EngineError::LinkNameTooLong {
                found: name.len(),
                name,
                max: LINK_NAME_LEN,
            });
        }
    }

    let topics = strategy.link_topics();
    let peers = resolve_peers(&link_config.subscribe, topics)?;
    let socket = runtime
        .block_on(UdpSocket::bind(link_config.bind))
        .map_err(|source| EngineError::LinkBind {
            bind: link_config.bind.to_string().into_boxed_str(),
            trading_engine: identity.to_string().into_boxed_str(),
            source,
        })?;
    info!(
        "link bound on {} — {} peer subscription(s)",
        link_config.bind,
        peers.len()
    );

    let token = link_config.token.as_deref().unwrap_or_default();
    Ok(Some(BoundLink {
        socket,
        identity: LinkIdentity {
            token_hash: LinkHash::of_name(token),
            strategy_hash: LinkHash::of_name(identity.strategy_id.as_str()),
            sender_te_hash: LinkHash::of_name(&identity.to_string()),
            // Wall-clock boot; restarted peer's seq=0 reads as restart, not stale stream.
            boot_ts_us: boot_stamp_us(),
        },
        guard: FrameGuard {
            token_hash: LinkHash::of_name(token),
            strategy_hash: LinkHash::of_name(identity.strategy_id.as_str()),
            schema_hash: schema_hash_of_fields(strategy.link_fields()),
        },
        peers,
        topic_count: TopicId::space_len(topics.len()),
        feature_count,
    }))
}
