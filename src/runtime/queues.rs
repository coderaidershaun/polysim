//! Input-queue fabric: one SPSC ring per queue id the registry issued, all built before any
//! producer exists so the hot thread owns every read end from the first message on.

use std::sync::Arc;

use rtrb::{Consumer, RingBuffer};

use crate::config::{BinanceMarket, VenueMarket};
use crate::hot::ingress::IngressQueues;
use crate::hot::spawn::{LinkQueueProducer, QueueProducer, SimTapGate};
use crate::ids::SourceId;
use crate::info;
use crate::msg::inbound::{InboundMessage, MarketTapItem};
use crate::registry::{ConnectionCategory, Registry};
use crate::shutdown::FatalSignal;

pub(super) struct InputQueues {
    pub ingress: IngressQueues,
    /// In producer_groups() order for spawn_adapters zip.
    pub groups: Vec<QueueProducer>,
    pub timer: QueueProducer,
    /// None with no link: block. Drop+count, not fatal-on-full.
    pub link: Option<LinkQueueProducer>,
    /// None unless execution actor exists. Engine-fed -> FATAL-on-full: engine can't keep up.
    pub exec: Option<QueueProducer>,
}

pub(super) fn build_input_queues(
    registry: &Registry,
    capacity: usize,
    fatal: &FatalSignal,
) -> InputQueues {
    let mut consumers: Vec<Option<Consumer<InboundMessage>>> =
        (0..registry.input_queue_count()).map(|_| None).collect();
    // Push in producer_groups() order so Vec aligns for spawn_adapters zip.
    let mut group_producers = Vec::with_capacity(registry.producer_groups().len());
    for group in registry.producer_groups() {
        let (producer, consumer) = RingBuffer::<InboundMessage>::new(capacity);
        consumers[usize::from(group.queue_id.0)] = Some(consumer);
        group_producers.push(QueueProducer::new(
            producer,
            fatal.clone(),
            group.queue_id,
            group.source_id,
        ));
    }

    let timer_queue_id = registry.timer_queue_id();
    let (timer_ring, timer_consumer) = RingBuffer::<InboundMessage>::new(capacity);
    consumers[usize::from(timer_queue_id.0)] = Some(timer_consumer);
    // Timer not a producer group -> source id one past last group.
    let timer_source_id = SourceId(registry.producer_groups().len() as u16);
    let timer_producer =
        QueueProducer::new(timer_ring, fatal.clone(), timer_queue_id, timer_source_id);

    let link_producer = registry.link_queue_id().map(|queue_id| {
        let (link_ring, link_consumer) = RingBuffer::<InboundMessage>::new(capacity);
        consumers[usize::from(queue_id.0)] = Some(link_consumer);
        LinkQueueProducer::new(link_ring, queue_id)
    });

    // One past last producer group + timer -> unique source id.
    let exec_producer = registry.exec_queue_id().map(|queue_id| {
        let (exec_ring, exec_consumer) = RingBuffer::<InboundMessage>::new(capacity);
        consumers[usize::from(queue_id.0)] = Some(exec_consumer);
        let source_id = SourceId(registry.producer_groups().len() as u16 + 1);
        QueueProducer::new(exec_ring, fatal.clone(), queue_id, source_id)
    });

    let consumers = consumers
        .into_iter()
        .map(|slot| slot.expect("every queue id assigned a ring"))
        .collect();
    InputQueues {
        ingress: IngressQueues::new(consumers),
        groups: group_producers,
        timer: timer_producer,
        link: link_producer,
        exec: exec_producer,
    }
}

pub(super) struct MarketTaps {
    pub trades: Consumer<MarketTapItem>,
    pub depth: Consumer<MarketTapItem>,
    pub gate: Arc<SimTapGate>,
}

/// # Panics
/// If either Binance Spot lane is missing or duplicated.
pub(super) fn attach_market_taps(
    registry: &Registry,
    capacity: usize,
    groups: Vec<QueueProducer>,
) -> (Vec<QueueProducer>, MarketTaps) {
    assert_eq!(
        groups.len(),
        registry.producer_groups().len(),
        "the zip below would silently drop a hot producer"
    );
    let gate = Arc::new(SimTapGate::new());
    let mut trades = None;
    let mut depth = None;
    let mut tapped = Vec::with_capacity(groups.len());

    for (group, producer) in registry.producer_groups().iter().zip(groups) {
        let Some(slot) = tapped_slot(group.market, group.category, &mut trades, &mut depth) else {
            tapped.push(producer);
            continue;
        };
        let (tap_producer, tap_consumer) = RingBuffer::<MarketTapItem>::new(capacity);
        info!(
            "market tap for input queue {} ({}) allocated {} slots of {} bytes",
            group.queue_id.0,
            group.category.as_str(),
            capacity,
            size_of::<MarketTapItem>()
        );
        *slot = Some(tap_consumer);
        tapped.push(producer.with_tap(tap_producer, Arc::clone(&gate)));
    }

    let taps = MarketTaps {
        trades: trades.expect("simulator requires a binance spot trades producer group"),
        depth: depth.expect("simulator requires a binance spot depth producer group"),
        gate,
    };
    (tapped, taps)
}

fn tapped_slot<'a>(
    market: VenueMarket,
    category: ConnectionCategory,
    trades: &'a mut Option<Consumer<MarketTapItem>>,
    depth: &'a mut Option<Consumer<MarketTapItem>>,
) -> Option<&'a mut Option<Consumer<MarketTapItem>>> {
    if market != VenueMarket::Binance(BinanceMarket::Spot) {
        return None;
    }
    let slot = match category {
        ConnectionCategory::Trades => trades,
        ConnectionCategory::Depth => depth,
        ConnectionCategory::Klines | ConnectionCategory::Market => return None,
    };
    assert!(
        slot.is_none(),
        "one simulator tap per lane: {} appears twice in the producer groups",
        category.as_str()
    );
    Some(slot)
}
