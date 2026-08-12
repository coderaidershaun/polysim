//! Config/registry venue-generalisation shape: one trading engine carries exactly one source, and
//! that source's fan-out to instruments, producer groups and input queues is asserted through the
//! real `from_yaml`/`build` path — one binance source is three connections onto one instrument, one
//! polymarket source is one connection onto the four fixed A/B-by-up/down slots. The shipped
//! reference config is asserted here too, since `deny_unknown_fields` makes it brittle to any
//! schema change.

use std::path::Path;

use polysim::config::{
    ConfigError, ExecutionMode, NoParams, PolySeries, TableKind, VenueMarket, VolumeThreshold,
};
use polysim::hot::strategy::{EngineView, StrategyConfig};
use polysim::ids::FIXED_SCALE;
use polysim::registry::{ConnectionCategory, Registry};
use polysim::time::DurationUs;

use crate::micro_strategy::{MicroRecorder, MicroRecorderParams};
use crate::poly_strategy::{PolyUpParams, PolyUpPublisher};

fn build_from(yaml: &str) -> Registry {
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(yaml).expect("document parses and validates");
    Registry::build(&config).expect("registry builds")
}

const POLY_SOURCE: &str = "  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  tracker: {}
";

const POLY_SLOT_SYMBOLS: [&str; 4] = [
    "btc-updown-5m-a-up",
    "btc-updown-5m-a-down",
    "btc-updown-5m-b-up",
    "btc-updown-5m-b-down",
];

fn config_with_source(source_block: &str) -> String {
    config_with_strategy(source_block, "  instruments: all\n")
}

fn config_with_strategy(source_block: &str, strategy_block: &str) -> String {
    document(
        source_block,
        strategy_block,
        "persistence:\n  dir: ./data\n",
    )
}

fn config_without_persistence(source_block: &str, strategy_block: &str) -> String {
    document(source_block, strategy_block, "")
}

fn document(source_block: &str, strategy_block: &str, persistence_block: &str) -> String {
    with_link(source_block, strategy_block, persistence_block, "")
}

fn with_link(
    source_block: &str,
    strategy_block: &str,
    persistence_block: &str,
    link_block: &str,
) -> String {
    format!(
        "engine:
  hot_core_id: 0
  spin_interval_us: 100000
queues:
  input_capacity: 65536
  persistence_capacity: 65536
source:
{source_block}strategy:
{strategy_block}{persistence_block}{link_block}logging:
  dir: ./logs
"
    )
}

fn link_block(bind: &str, extra: &str) -> String {
    format!("link:\n  bind: \"{bind}\"\n{extra}")
}

fn config_with_link(link_block: &str) -> String {
    with_link(
        BINANCE_PERP_SOURCE,
        "  instruments: all\n",
        "persistence:\n  dir: ./data\n",
        link_block,
    )
}

const BINANCE_PERP_SOURCE: &str = "  exchange: binance
  max_exposure_quote: 500
  market: perpetual
  base: BTC
  quote: USDT
  tracker: {}
";

/// One polymarket source expands to the four fixed A/B x up/down slots, each a distinct venue
/// symbol carrying a dense id in config order, none of them subscribing to klines, all on one queue.
#[test]
fn polymarket_source_fans_out_to_four_slots() {
    let registry = build_from(&config_with_source(POLY_SOURCE));
    let rows = registry.instruments();
    let symbols: Vec<&str> = rows.iter().map(|row| row.venue_symbol.as_ref()).collect();
    assert_eq!(
        symbols, POLY_SLOT_SYMBOLS,
        "four distinct slot symbols in order"
    );
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            usize::from(row.instrument_id.0),
            index,
            "dense id in config order"
        );
        assert_eq!(row.market, VenueMarket::Polymarket(PolySeries::BtcUpDown5m));
        assert!(row.kline_intervals.is_empty(), "polymarket has no klines");
        assert!(!row.subscriptions.klines, "poly klines subscription is off");
    }

    let groups = registry.producer_groups();
    assert_eq!(groups.len(), 1, "one connection for all four slots");
    assert_eq!(groups[0].category, ConnectionCategory::Market);
    assert_eq!(
        groups[0].instruments.len(),
        4,
        "all four slots share the queue"
    );
    assert_eq!(registry.input_queue_count(), 2, "one producer + the timer");
}

/// "One source" is not "one connection": a single binance source is ONE instrument served by three
/// per-category connections (trades/depth/klines), so it is three producer groups plus the timer.
/// The polymarket counterpart — one connection, four slots, two queues — is asserted above.
#[test]
fn a_binance_source_is_three_connections_on_one_instrument() {
    let registry = build_from(&config_with_source(BINANCE_PERP_SOURCE));
    let ids: Vec<u16> = registry
        .instruments()
        .iter()
        .map(|row| row.instrument_id.0)
        .collect();
    assert_eq!(ids, [0], "one binance source is one instrument row");

    let categories: Vec<ConnectionCategory> = registry
        .producer_groups()
        .iter()
        .map(|group| group.category)
        .collect();
    assert_eq!(
        categories,
        [
            ConnectionCategory::Trades,
            ConnectionCategory::Depth,
            ConnectionCategory::Klines
        ],
        "three per-category connections onto the one instrument"
    );
    assert_eq!(
        registry.input_queue_count(),
        4,
        "three producers + the timer"
    );
}

/// One trading engine takes exactly ONE source, so a config still carrying the old plural
/// `sources:` list must fail loud naming the key and pointing at the singular one — parsing on and
/// leaving the engine bound to no source would be the worst of both.
#[test]
fn a_stale_plural_sources_list_is_rejected_by_name() {
    let stale = "engine:
  hot_core_id: 0
  spin_interval_us: 100000
queues:
  input_capacity: 65536
  persistence_capacity: 65536
sources:
  - exchange: binance
    max_exposure_quote: 500
    market: perpetual
    base: BTC
    quote: USDT
    tracker: {}
strategy:
  instruments: all
persistence:
  dir: ./data
logging:
  dir: ./logs
";
    let error = polysim::config::Config::<NoParams>::from_yaml(stale)
        .expect_err("a plural sources list must reject");
    assert!(
        matches!(&error, ConfigError::Parse { detail }
            if detail.contains("unknown field `sources`") && detail.contains("source,")),
        "error names the stale key and the singular one that replaced it, got: {error}"
    );
}

/// A trading engine that only computes signals and ships them over the link should pay for no writer
/// thread, no ring and no run directory, so the whole `persistence:` block is optional. And because
/// `strategy.tables` is the lib's authority over which tables exist at all, naming tables with no
/// block to write them into is a contradiction: it must fail loud rather than record nothing.
#[test]
fn persistence_is_optional_and_naming_tables_without_it_is_rejected() {
    let headless = config_without_persistence(BINANCE_PERP_SOURCE, "  instruments: all\n");
    let config: polysim::config::Config = polysim::config::Config::from_yaml(&headless)
        .expect("an omitted persistence block is legal");
    assert!(
        config.persistence.is_none(),
        "an omitted block means no persistence at all, not a defaulted directory"
    );
    assert!(
        config.strategy.tables.is_empty(),
        "no persistence, no tables"
    );

    let contradiction =
        config_without_persistence(BINANCE_PERP_SOURCE, "  tables: [features, trades]\n");
    let error = polysim::config::Config::<NoParams>::from_yaml(&contradiction)
        .expect_err("tables with nowhere to land must reject");
    assert!(
        matches!(&error, ConfigError::TablesWithoutPersistence { tables }
            if tables.as_ref() == "features, trades"),
        "error names every table that has nowhere to land, got: {error}"
    );

    let with_block: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_source(BINANCE_PERP_SOURCE))
            .expect("a persistence block still parses");
    assert_eq!(
        with_block
            .persistence
            .as_ref()
            .expect("the block is present")
            .dir,
        Path::new("./data")
    );
}

/// Polymarket serves trades and the book on one combined channel, so a partial subscriptions block
/// is a config promise the venue arm can't honour — the false flags would be silently void (`trades:
/// false` would still record trades). Build must reject the mixed combo and name the false flag(s).
#[test]
fn a_partial_poly_subscription_is_rejected() {
    let one_false = "  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  subscriptions:
    trades: false
  tracker: {}
";
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_source(one_false))
            .expect("document parses");
    let error = Registry::build(&config).expect_err("a mixed subscription combo must reject");
    assert!(
        matches!(
            &error,
            ConfigError::Invalid { field, value, .. }
                if *field == "source.subscriptions" && value.as_ref() == "trades: false"
        ),
        "error names the false flag, got: {error}"
    );

    let two_false = "  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  subscriptions:
    book_updates: false
    book_snapshots: false
  tracker: {}
";
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_source(two_false))
            .expect("document parses");
    let error = Registry::build(&config).expect_err("a mixed subscription combo must reject");
    assert!(
        matches!(
            &error,
            ConfigError::Invalid { field, value, .. }
                if *field == "source.subscriptions"
                    && value.as_ref() == "book_updates: false, book_snapshots: false"
        ),
        "error names both false flags, got: {error}"
    );
}

/// The uniform combos stay valid: all-true subscribes the combined channel; all-false builds four
/// dead rows and no group (silent-but-harmless, symmetric with a fully-unsubscribed binance source).
#[test]
fn uniform_poly_subscriptions_build() {
    let all_true = "  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  subscriptions:
    trades: true
    book_updates: true
    book_snapshots: true
  tracker: {}
";
    let registry = build_from(&config_with_source(all_true));
    assert_eq!(registry.producer_groups().len(), 1, "one combined group");

    let all_false = "  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  subscriptions:
    trades: false
    book_updates: false
    book_snapshots: false
  tracker: {}
";
    let registry = build_from(&config_with_source(all_false));
    assert_eq!(registry.instruments().len(), 4, "dead rows still built");
    assert!(registry.producer_groups().is_empty(), "no group, no socket");
    assert_eq!(registry.input_queue_count(), 1, "timer queue only");
}

/// An explicit `strategy.instruments` symbol that names no built instrument is a config promise that
/// would silently record nothing — build must reject it loudly and name the symbol. A symbol
/// that does resolve (case-insensitively) still builds.
#[test]
fn unknown_strategy_instrument_is_rejected() {
    let unknown = config_with_strategy(BINANCE_PERP_SOURCE, "  instruments:\n    - ethusdt\n");
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&unknown).expect("document parses");
    let error = Registry::build(&config).expect_err("an unmatched symbol must reject");
    assert!(
        matches!(&error, ConfigError::UnknownStrategyInstrument { symbol } if symbol.as_ref() == "ethusdt"),
        "error names the unmatched symbol, got: {error}"
    );

    // The venue symbol is lowercase `btcusdt`; an upper-case listing still resolves and builds.
    let matched = config_with_strategy(BINANCE_PERP_SOURCE, "  instruments:\n    - BTCUSDT\n");
    build_from(&matched);
}

/// Every derived window, buffer length and per-second rescale in a strategy is a function of the
/// spin interval, so a defaulted one would silently disagree with whatever the strategy assumed.
/// Omitting it must fail loud and name the field; the existing range check still bounds it.
#[test]
fn spin_interval_is_mandatory_and_bounded() {
    let missing = "engine:
  hot_core_id: 0
queues:
  input_capacity: 65536
  persistence_capacity: 65536
source:
"
    .to_owned()
        + BINANCE_PERP_SOURCE
        + "strategy:\n  instruments: all\npersistence:\n  dir: ./data\nlogging:\n  dir: ./logs\n";
    let error = polysim::config::Config::<NoParams>::from_yaml(&missing)
        .expect_err("an omitted spin_interval_us must reject");
    assert!(
        matches!(&error, ConfigError::Parse { detail } if detail.contains("spin_interval_us")),
        "error names the missing field, got: {error}"
    );

    let too_slow = config_with_source(BINANCE_PERP_SOURCE)
        .replace("spin_interval_us: 100000", "spin_interval_us: 90000000");
    let error = polysim::config::Config::<NoParams>::from_yaml(&too_slow)
        .expect_err("90s is past the 60s ceiling");
    assert!(
        matches!(&error, ConfigError::EngineFieldRange { field, value, .. }
            if *field == "spin_interval_us" && *value == 90_000_000),
        "error names the field and value, got: {error}"
    );
}

/// `warmup_secs` defaults to 10 when omitted (matching the shipped configs) and rejects a value
/// past the 1h ceiling — a run left mostly suppressed reads as minutes typed into a seconds field.
#[test]
fn warmup_secs_defaults_and_range_check() {
    let defaulted: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_source(BINANCE_PERP_SOURCE))
            .expect("omitting warmup_secs is valid");
    assert_eq!(
        defaulted.engine.warmup_secs, 10,
        "warmup_secs defaults to 10"
    );

    let engine_block =
        "engine:\n  hot_core_id: 0\n  spin_interval_us: 100000\n  warmup_secs: 7200\n";
    let over_ceiling = format!(
        "{engine_block}queues:\n  input_capacity: 65536\n  persistence_capacity: 65536\nsource:\n{BINANCE_PERP_SOURCE}strategy:\n  instruments: all\npersistence:\n  dir: ./data\nlogging:\n  dir: ./logs\n"
    );
    let error = polysim::config::Config::<NoParams>::from_yaml(&over_ceiling)
        .expect_err("7200s is past the ceiling");
    assert!(
        matches!(&error, ConfigError::EngineFieldRange { field, value, .. }
            if *field == "warmup_secs" && *value == 7200),
        "error names the field and value, got: {error}"
    );

    let zero = "engine:\n  hot_core_id: 0\n  spin_interval_us: 100000\n  warmup_secs: 0\n";
    let disabled = format!(
        "{zero}queues:\n  input_capacity: 65536\n  persistence_capacity: 65536\nsource:\n{BINANCE_PERP_SOURCE}strategy:\n  instruments: all\npersistence:\n  dir: ./data\nlogging:\n  dir: ./logs\n"
    );
    let parsed: polysim::config::Config =
        polysim::config::Config::from_yaml(&disabled).expect("0 disables warmup, legal");
    assert_eq!(parsed.engine.warmup_secs, 0, "0 is accepted");
}

#[test]
fn the_drain_deadline_has_a_finite_operational_ceiling() {
    let over_ceiling = config_with_source(BINANCE_PERP_SOURCE).replace(
        "spin_interval_us: 100000",
        "spin_interval_us: 100000\n  drain_deadline_ms: 86400001",
    );
    let error = refusal(&over_ceiling);
    assert!(
        matches!(
            &error,
            ConfigError::EngineFieldRange {
                field: "drain_deadline_ms",
                value: 86_400_001,
                ..
            }
        ),
        "an effectively unbounded coordinated drain must be refused by name, got {error}"
    );

    let at_ceiling = over_ceiling.replace("86400001", "86400000");
    polysim::config::Config::<NoParams>::from_yaml(&at_ceiling)
        .expect("the 24-hour operational ceiling is inclusive");
}

/// A volume-bar target is written either as the word `klines` or as a whole-dollar integer, so the
/// hand-written visitor must accept both and reject everything else by name. The klines form also
/// promises a trailing 1m average, which only holds if the source keeps 1440 closed 1m candles —
/// build must refuse the promise it cannot keep rather than quietly averaging a shorter window.
#[test]
fn volume_bar_thresholds_parse_in_both_forms() {
    let klines = "  exchange: binance
  max_exposure_quote: 500
  market: perpetual
  base: BTC
  quote: USDT
  tracker:
    candles: { keep: 1440 }
    volume_bars:
      threshold: klines
      keep: 1440
";
    let registry = build_from(&config_with_source(klines));
    let spec = registry.instruments()[0]
        .tracker
        .volume_bars
        .as_ref()
        .expect("volume_bars parsed");
    assert_eq!(spec.threshold, VolumeThreshold::Klines);
    assert!(spec.sampled.is_none(), "an absent sampled block is legal");

    let fixed = "  exchange: binance
  max_exposure_quote: 500
  market: perpetual
  base: BTC
  quote: USDT
  tracker:
    volume_bars:
      threshold: 250000
      keep: 512
      sampled: { fields: [best_bid], window: 256 }
";
    let registry = build_from(&config_with_source(fixed));
    let spec = registry.instruments()[0]
        .tracker
        .volume_bars
        .as_ref()
        .expect("volume_bars parsed");
    assert_eq!(spec.threshold, VolumeThreshold::Fixed(250_000));
    assert_eq!(
        spec.sampled.as_ref().expect("sampled block parsed").window,
        256
    );

    let short_candles = klines.replace("keep: 1440 }", "keep: 720 }");
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_source(&short_candles))
            .expect("document parses");
    let error = Registry::build(&config).expect_err("720 candles cannot back a 1440 average");
    assert!(
        matches!(&error, ConfigError::Invalid { field, value, .. }
            if *field == "source.tracker.candles.keep" && value.as_ref() == "720"),
        "error names the retention that falls short, got: {error}"
    );

    // A klines target averages 1m candles, which polymarket lacks — build must refuse rather than
    // leave a clock that would sit silently dormant forever.
    let poly_klines = "  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  tracker:
    volume_bars:
      threshold: klines
      keep: 64
";
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_source(poly_klines))
            .expect("document parses");
    let error = Registry::build(&config).expect_err("polymarket has no candles to average");
    assert!(
        matches!(&error, ConfigError::Invalid { field, value, .. }
            if *field == "source.tracker.volume_bars.threshold" && value.as_ref() == "klines"),
        "error names the impossible klines target, got: {error}"
    );
}

/// `deny_unknown_fields` reaches inside the new block, and a threshold that is neither form fails
/// at parse naming both — a startup typo must never become a silently different bar size.
#[test]
fn a_malformed_volume_bars_block_is_rejected() {
    let unknown_key = "  exchange: binance
  max_exposure_quote: 500
  market: perpetual
  base: BTC
  quote: USDT
  tracker:
    volume_bars:
      threshold: 250000
      keep: 512
      windwo: 256
";
    let error = polysim::config::Config::<NoParams>::from_yaml(&config_with_source(unknown_key))
        .expect_err("an unknown key inside volume_bars must reject");
    assert!(
        matches!(&error, ConfigError::Parse { detail } if detail.contains("windwo")),
        "error names the unknown field, got: {error}"
    );

    let bad_scalar = unknown_key
        .replace("threshold: 250000", "threshold: kline")
        .replace("      windwo: 256\n", "");
    let error = polysim::config::Config::<NoParams>::from_yaml(&config_with_source(&bad_scalar))
        .expect_err("a threshold that is neither form must reject");
    assert!(
        matches!(&error, ConfigError::Parse { detail } if detail.contains("klines")),
        "error names the accepted forms, got: {error}"
    );
}

/// Generic over the strategy's `params:` type, so a shipped config is read through the same typed
/// pass its own binary uses — parsing it as [`NoParams`] would skip its knobs entirely.
fn load<P: serde::de::DeserializeOwned + Default>(path: &str) -> polysim::config::Config<P> {
    polysim::config::Config::load(Path::new(path))
        .unwrap_or_else(|error| panic!("{path} must load: {error}"))
}

/// The config the binary header tells a reader to run must parse, validate and build a registry,
/// and carry the tables its strategy actually writes — a config naming no table its strategy emits
/// records nothing at all.
#[test]
fn the_shipped_reference_config_runs_its_strategy() {
    let micro = load::<MicroRecorderParams>(
        "strategies/strat-micro-recorder/te-binance-spot-btcusdt/config.yaml",
    );
    assert_eq!(
        micro.strategy.tables,
        vec![
            TableKind::Features,
            TableKind::LinkFrames,
            TableKind::Orders,
            TableKind::Fills
        ],
        "features, the consumed link tape, and the audit trail — a live run that omits orders and \
         fills keeps no record of what it did with real money, and nothing downstream can tell that \
         apart from a run that did nothing"
    );
    let execution = micro
        .execution
        .as_ref()
        .expect("the shipped config carries an execution block");
    assert_eq!(
        execution.mode,
        ExecutionMode::Off,
        "the reference config ships disarmed; an operator may opt into deterministic sim or live"
    );
    let link = micro
        .link
        .as_ref()
        .expect("the shipped config binds a link");
    let peer = match link.subscribe.as_slice() {
        [only] => only,
        other => panic!("this recorder consumes exactly its polymarket peer, got {other:?}"),
    };
    assert_eq!(
        peer.topics.iter().map(AsRef::as_ref).collect::<Vec<&str>>(),
        vec!["poly_up"],
        "naming the topic rather than omitting it — an absent list asks for the peer's whole feed"
    );
    assert_eq!(
        Registry::build(&micro)
            .expect("micro recorder registry builds")
            .instruments()
            .len(),
        1,
        "one source, one binance spot instrument"
    );
    // Construction is the strategy's own validation pass — a shipped params value its asserts
    // reject (a non-positive Guéant risk budget, say) must fail here, not at first launch.
    let engine = EngineView {
        spin_interval: DurationUs::from_micros(micro.engine.spin_interval_us as i64),
    };
    let _ = MicroRecorder::from_spec(&micro.strategy, engine);
}

/// The polymarket half of the pair, and the facts its own config must state: it ships disarmed
/// twice over, it collects nothing beyond the rotations lineage its peer's data cannot do without,
/// its quoting stops before the engine's own window gate would refuse it, and it binds a port its
/// peer does not. Both engines ship configured for one box, where a collision is a startup failure
/// an operator has to diagnose rather than a test catching it.
#[test]
fn the_polymarket_config_ships_disarmed_and_binds_clear_of_its_peer() {
    let publisher = load::<PolyUpParams>(
        "strategies/strat-micro-recorder/te-polymarket-btc-updown-5m/config.yaml",
    );
    assert!(
        publisher.strategy.tables.is_empty(),
        "collection is the binance TE's job; arming execution must restore [orders, fills] — \
         an engine that can place real orders records what it did with them"
    );
    assert!(
        publisher.persistence.is_some(),
        "the rotations lineage still needs the writer thread: it is the only map from the \
         role-keyed poly_* features back to a condition id and window"
    );
    let execution = publisher
        .execution
        .as_ref()
        .expect("the shipped config carries an execution block");
    assert_eq!(
        execution.mode,
        ExecutionMode::Off,
        "polymarket ships disarmed; live is the operator's decision and there is no sim venue"
    );
    assert!(
        !publisher.strategy.params.enabled,
        "the strategy's own switch ships off too — arming execution for any other reason must not \
         start a market maker by omission"
    );
    // The engine refuses a quote inside `quote_stop_margin_ms` of the close and sweeps what is
    // resting. A strategy whose own stop landed at or after that margin would declare a quote every
    // spin for the engine to refuse, and the refusal stream would bury anything else being said.
    assert!(
        u64::from(publisher.strategy.params.quote_stop_lead_ms) > execution.quote_stop_margin_ms,
        "the strategy stops quoting {}ms before the close, inside the engine's own {}ms margin",
        publisher.strategy.params.quote_stop_lead_ms,
        execution.quote_stop_margin_ms
    );
    let link = publisher
        .link
        .as_ref()
        .expect("the publisher must bind a link, it has no other output");
    assert!(
        link.subscribe.is_empty(),
        "the publisher serves and consumes nothing"
    );
    let registry = Registry::build(&publisher).expect("publisher registry builds");
    assert_eq!(
        registry.instruments().len(),
        4,
        "one polymarket source, four A/B x up/down slots"
    );
    // Eight of the twenty published slots are (A, k) fits, and without a reach histogram to fit they
    // are NaN for the life of the run while everything else keeps working — a deleted block would
    // read downstream as a series that never trades, not as a misconfiguration.
    assert!(
        registry
            .instruments()
            .iter()
            .all(|row| row.tracker.intensity.is_some()),
        "the publisher cannot fit trade intensity without the tracker's reach histogram"
    );
    let engine = EngineView {
        spin_interval: DurationUs::from_micros(publisher.engine.spin_interval_us as i64),
    };
    let _ = PolyUpPublisher::from_spec(&publisher.strategy, engine);
    // The one arithmetic relation between these numbers that nothing else checks. An outcome share
    // cannot be worth more than a dollar, so `order_shares` IS the worst-case notional — set it
    // above the single-order ceiling and every place is refused as oversized, on a config that
    // otherwise validates.
    assert!(
        publisher.strategy.params.order_shares <= execution.max_order_notional_quote,
        "{} shares can cost up to ${} at settlement, past the ${} single-order ceiling",
        publisher.strategy.params.order_shares,
        publisher.strategy.params.order_shares,
        execution.max_order_notional_quote
    );

    let consumer = load::<MicroRecorderParams>(
        "strategies/strat-micro-recorder/te-binance-spot-btcusdt/config.yaml",
    );
    let consumer_link = consumer.link.as_ref().expect("the consumer binds a link");
    assert_ne!(
        link.bind, consumer_link.bind,
        "two trading engines of one strategy ship runnable side by side on one box"
    );
    assert_eq!(
        consumer_link.subscribe.first().map(|peer| peer.address),
        Some(link.bind),
        "the consumer's peer address must be the address the publisher actually binds"
    );
}

/// The exposure ceiling is a risk budget, so it is stated per source block and never defaulted: an
/// omitted one must fail the run by name rather than silently adopt a number nobody chose. It
/// reaches every row the block expands to as an exact quote mantissa — the four poly slots are
/// budgeted separately, not as one pot.
#[test]
fn the_exposure_ceiling_is_mandatory_and_reaches_every_row() {
    for (source, rows) in [(BINANCE_PERP_SOURCE, 1), (POLY_SOURCE, 4)] {
        let registry = build_from(&config_with_source(source));
        let budgets: Vec<i64> = registry
            .instruments()
            .iter()
            .map(|row| row.max_exposure_quote)
            .collect();
        assert_eq!(
            budgets,
            vec![500 * FIXED_SCALE; rows],
            "every row the block expands to carries its 500 quote units, exactly"
        );
    }

    let omitted = "  exchange: binance
  market: perpetual
  base: BTC
  quote: USDT
  tracker: {}
";
    let error = polysim::config::Config::<NoParams>::from_yaml(&config_with_source(omitted))
        .expect_err("an omitted exposure budget must reject");
    assert!(
        matches!(&error, ConfigError::Parse { detail } if detail.contains("max_exposure_quote")),
        "error names the missing field, got: {error}"
    );
}

/// A strategy's typed `params:` block deserializes in the same pass with `deny_unknown_fields`
/// intact: a good block parses to the typed value, an unknown key inside `params` fails naming the
/// field, and an omitted block yields the strategy's default. This is the seam that lets a strategy
/// carry its own knobs without the lib knowing the shape.
#[test]
fn strategy_params_parse_typed_and_reject_unknown_keys() {
    #[derive(serde::Deserialize, Debug, Clone, Default)]
    #[serde(deny_unknown_fields)]
    struct FakeParams {
        threshold: f64,
    }

    let with_params = config_with_strategy(BINANCE_PERP_SOURCE, "  params:\n    threshold: 1.5\n");
    let config = polysim::config::Config::<FakeParams>::from_yaml(&with_params)
        .expect("typed params parse in the single pass");
    assert_eq!(config.strategy.params.threshold, 1.5);

    let unknown_key = config_with_strategy(BINANCE_PERP_SOURCE, "  params:\n    threshld: 1.5\n");
    let error = polysim::config::Config::<FakeParams>::from_yaml(&unknown_key)
        .expect_err("an unknown key inside params must reject");
    assert!(
        matches!(&error, ConfigError::Parse { detail } if detail.contains("threshld")),
        "error names the unknown params field, got: {error}"
    );

    let omitted = config_with_strategy(BINANCE_PERP_SOURCE, "  instruments: all\n");
    let config = polysim::config::Config::<FakeParams>::from_yaml(&omitted)
        .expect("an omitted params block is legal");
    assert_eq!(
        config.strategy.params.threshold, 0.0,
        "omitted params yields the strategy default"
    );
}

/// The link has no authentication, so the bind address IS the security boundary. A bind reachable
/// from outside a private network must fail the run by name — anyone who can reach that port can
/// stop the engine and inject signals into the strategy — and `allow_public_bind` must be the only
/// way past it, so the exposure is always somebody's stated decision.
#[test]
fn a_public_link_bind_is_a_startup_error_unless_allowed() {
    let private = [
        "127.0.0.1:9310",
        "10.1.2.3:9310",
        "172.16.0.1:9310",
        "192.168.1.4:9310",
        "100.64.0.3:9310",
        "[::1]:9310",
        "[fd00::1]:9310",
    ];
    for bind in private {
        let yaml = config_with_link(&link_block(bind, ""));
        polysim::config::Config::<NoParams>::from_yaml(&yaml)
            .unwrap_or_else(|error| panic!("{bind} is a private bind: {error}"));
    }

    let public = [
        "0.0.0.0:9310",
        "8.8.8.8:9310",
        "172.32.0.1:9310",
        "100.128.0.1:9310",
        "[::]:9310",
        "[2001:db8::1]:9310",
    ];
    for bind in public {
        let yaml = config_with_link(&link_block(bind, ""));
        let error = polysim::config::Config::<NoParams>::from_yaml(&yaml)
            .err()
            .unwrap_or_else(|| panic!("{bind} must be refused"));
        assert!(
            matches!(error, ConfigError::PublicLinkBind { .. }),
            "{bind} refused for the right reason, got {error}"
        );
        let allowed = config_with_link(&link_block(bind, "  allow_public_bind: true\n"));
        polysim::config::Config::<NoParams>::from_yaml(&allowed)
            .unwrap_or_else(|error| panic!("{bind} allowed explicitly: {error}"));
    }
}

/// The link's input queue exists only when its actor does, and it lands after the timer — a queue id
/// is a position in the ingress list, and `wire` zips producers back against exactly that order.
#[test]
fn the_link_adds_one_input_queue_only_when_configured() {
    let without = build_from(&config_with_source(BINANCE_PERP_SOURCE));
    assert_eq!(
        without.link_queue_id(),
        None,
        "no link block, no link queue"
    );
    assert_eq!(
        without.input_queue_count(),
        4,
        "three producers + the timer"
    );

    let with = build_from(&config_with_link(&link_block("127.0.0.1:9310", "")));
    assert_eq!(
        with.timer_queue_id().0,
        3,
        "the timer still follows the three producer groups"
    );
    assert_eq!(
        with.link_queue_id().map(|id| id.0),
        Some(4),
        "the link takes the queue after the timer"
    );
    assert_eq!(
        with.input_queue_count(),
        5,
        "three producers + the timer + the link"
    );
}

/// The `execution:` block, and the three things about it that are easy to get silently wrong: which
/// spellings of `mode` exist, that a disarmed block is still fully validated, and that the queue it
/// costs appears only when an actor will exist to feed it.
/// Every field the block has no default for, as pairs so one test can vary a single value without
/// restating the rest.
const EXECUTION_REQUIRED: [(&str, &str); 6] = [
    ("min_base_balance", "0.001"),
    ("min_quote_balance", "10.0"),
    ("max_order_notional_quote", "25.0"),
    ("max_quote_distance_bps", "50.0"),
    ("max_book_age_ms", "2000"),
    ("max_session_loss_quote", "5.0"),
];

fn config_with_execution(block: &str) -> String {
    config_with_execution_on(BINANCE_PERP_SOURCE, block)
}

fn config_with_execution_on(source_block: &str, block: &str) -> String {
    format!(
        "engine:
  hot_core_id: 0
  spin_interval_us: 100000
queues:
  input_capacity: 65536
  persistence_capacity: 65536
source:
{source_block}strategy:
  instruments: all
persistence:
  dir: ./data
{block}logging:
  dir: ./logs
"
    )
}

const SIM_BLOCK: &str = "  sim:
    order_entry_latency_ms: 15
    ack_latency_ms: 15
    max_market_data_delay_ms: 1000
    heartbeat_ms: 100
    opening_base_balance: 0.01
    opening_quote_balance: 1000.0
    maker_fee_bps: 10.0
";

const BINANCE_SPOT_SOURCE: &str = "  exchange: binance
  max_exposure_quote: 500
  market: spot
  base: BTC
  quote: USDT
  tracker: {}
";

fn execution_block(mode: &str) -> String {
    execution_block_overriding(mode, "", "")
}

fn execution_block_with(mode: &str, extra: &str) -> String {
    format!("{}{extra}", execution_block(mode))
}

/// The same block with one field's value replaced, so a rejection test states only what it changed.
fn execution_block_overriding(mode: &str, field: &str, value: &str) -> String {
    let mut block = format!("execution:\n  mode: {mode}\n");
    for (name, default) in EXECUTION_REQUIRED {
        let stated = if name == field { value } else { default };
        block.push_str(&format!("  {name}: {stated}\n"));
    }
    block
}

#[test]
fn every_execution_mode_parses_and_only_the_armed_ones_cost_a_queue() {
    let modes = [
        ("off", BINANCE_PERP_SOURCE, "", ExecutionMode::Off, false),
        ("live", BINANCE_PERP_SOURCE, "", ExecutionMode::Live, true),
        (
            "sim",
            BINANCE_SPOT_SOURCE,
            SIM_BLOCK,
            ExecutionMode::Sim,
            true,
        ),
    ];
    for (spelling, source, extra, expected, expects_queue) in modes {
        let yaml = config_with_execution_on(source, &execution_block_with(spelling, extra));
        let config: polysim::config::Config =
            polysim::config::Config::from_yaml(&yaml).expect("the block parses and validates");
        assert_eq!(
            config.execution.as_ref().map(|block| block.mode),
            Some(expected),
            "{spelling} is the spelling an operator types"
        );
        let registry = Registry::build(&config).expect("registry builds");
        assert_eq!(
            registry.exec_queue_id().is_some(),
            expects_queue,
            "{spelling}: the exec input queue exists exactly when an edge will feed it"
        );
        assert_eq!(
            registry.input_queue_count(),
            if expects_queue { 5 } else { 4 },
            "{spelling}: three producer groups + the timer, plus execution when it is armed"
        );
    }
}

#[test]
fn every_reason_a_simulated_config_is_refused_names_itself() {
    let accepted =
        config_with_execution_on(BINANCE_SPOT_SOURCE, &execution_block_with("sim", SIM_BLOCK));
    polysim::config::Config::<NoParams>::from_yaml(&accepted)
        .expect("binance spot with all three market streams and a sim block is the valid shape");

    let no_block = config_with_execution_on(BINANCE_SPOT_SOURCE, &execution_block_with("sim", ""));
    assert!(
        matches!(
            refusal(&no_block),
            ConfigError::Invalid {
                field: "execution.sim",
                ..
            }
        ),
        "mode: sim with no sim block leaves every venue assumption at a default nobody chose"
    );

    for spelling in ["off", "live"] {
        let live_block = config_with_execution_on(
            BINANCE_SPOT_SOURCE,
            &execution_block_with(spelling, SIM_BLOCK),
        );
        assert!(
            matches!(
                refusal(&live_block),
                ConfigError::Invalid {
                    field: "execution.sim",
                    ..
                }
            ),
            "a sim block under {spelling} is an operator who believes they are paper trading"
        );
    }

    let perpetual =
        config_with_execution_on(BINANCE_PERP_SOURCE, &execution_block_with("sim", SIM_BLOCK));
    assert!(
        matches!(
            refusal(&perpetual),
            ConfigError::SimulatedExecutionMarket {
                market: "perpetual"
            }
        ),
        "the fill model is built on spot's depth granularity and aggregate trades"
    );

    let poly_sim = refusal(&config_with_execution_on(
        POLY_SOURCE,
        &execution_block_with("sim", SIM_BLOCK),
    ));
    let ConfigError::ExecutionModeUnsupported {
        venue,
        mode,
        supported,
    } = &poly_sim
    else {
        panic!("sim on a venue with no simulated edge must be refused by name, got {poly_sim}");
    };
    assert_eq!(
        (*venue, *mode, supported.as_ref()),
        ("polymarket", "sim", "off, live"),
        "the refusal names the venue, the mode it cannot serve, and what it can"
    );

    for (field, missing) in [
        ("trades", "trades"),
        ("book_updates", "book_updates"),
        ("book_snapshots", "book_snapshots"),
    ] {
        let source = format!("{BINANCE_SPOT_SOURCE}  subscriptions:\n    {field}: false\n");
        let yaml = config_with_execution_on(&source, &execution_block_with("sim", SIM_BLOCK));
        let error = refusal(&yaml);
        let ConfigError::SimulatedExecutionSubscriptions { missing: named } = &error else {
            panic!("disabling {field} must be refused by name, got {error}");
        };
        assert_eq!(
            named.as_ref(),
            missing,
            "the refusal names the stream the venue would have matched against"
        );
    }
}

#[test]
fn the_shipped_sim_defaults_are_a_legal_budget_on_their_own() {
    let bare = config_with_execution_on(
        BINANCE_SPOT_SOURCE,
        &execution_block_with("sim", "  sim: {}\n"),
    );
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&bare).expect("the ratified defaults validate together");
    let sim = config
        .execution
        .as_ref()
        .and_then(|execution| execution.sim.as_ref())
        .expect("mode sim carries a sim block");
    assert_eq!(sim.order_entry_latency_ms, 15);
    assert_eq!(sim.ack_latency_ms, 15);
    assert_eq!(sim.max_market_data_delay_ms, 1_000);
    assert_eq!(sim.heartbeat_ms, 100);
}

#[test]
fn a_one_second_spin_has_a_3030ms_budget_and_fits_the_default_timeout() {
    let yaml = config_with_execution_on(
        BINANCE_SPOT_SOURCE,
        &execution_block_with("sim", "  sim: {}\n"),
    )
    .replace("spin_interval_us: 100000", "spin_interval_us: 1000000");

    polysim::config::Config::<NoParams>::from_yaml(&yaml)
        .expect("3030ms is strictly inside the default 5000ms timeout");

    let tight = yaml.replace(
        "execution:\n  mode: sim\n",
        "execution:\n  mode: sim\n  inflight_timeout_ms: 3030\n",
    );
    let error = refusal(&tight);
    let ConfigError::Invalid {
        field: "execution.sim",
        value,
        ..
    } = &error
    else {
        panic!("the budget refusal must name the sim block, got {error}");
    };
    assert_eq!(
        value.as_ref(),
        "3030",
        "the operator is told the millisecond sum they have to clear, not which field to guess at"
    );
    let just_inside = yaml.replace(
        "execution:\n  mode: sim\n",
        "execution:\n  mode: sim\n  inflight_timeout_ms: 3031\n",
    );
    polysim::config::Config::<NoParams>::from_yaml(&just_inside)
        .expect("one millisecond beyond the worst legal answer is sufficient");
}

#[test]
fn zero_opening_balances_and_zero_maker_fee_are_valid_simulation_inputs() {
    let sim = SIM_BLOCK
        .replace("opening_base_balance: 0.01", "opening_base_balance: 0")
        .replace("opening_quote_balance: 1000.0", "opening_quote_balance: 0")
        .replace("maker_fee_bps: 10.0", "maker_fee_bps: 0");
    let yaml = config_with_execution_on(BINANCE_SPOT_SOURCE, &execution_block_with("sim", &sim));
    polysim::config::Config::<NoParams>::from_yaml(&yaml)
        .expect("an empty account and fee-free venue are useful simulator cases");
}

fn refusal(yaml: &str) -> ConfigError {
    polysim::config::Config::<NoParams>::from_yaml(yaml).expect_err("this document must be refused")
}

/// An ABSENT block and `mode: off` are different statements, and the difference is the point: the
/// first is an engine that was never meant to trade, the second one configured and disarmed. Neither
/// spawns anything, so the only observable difference is whether the limits are there to arm.
#[test]
fn an_absent_execution_block_is_not_the_same_as_a_disarmed_one() {
    let absent: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_execution("")).expect("parses");
    assert!(absent.execution.is_none());
    assert!(
        Registry::build(&absent)
            .expect("registry builds")
            .exec_queue_id()
            .is_none()
    );

    let disarmed: polysim::config::Config =
        polysim::config::Config::from_yaml(&config_with_execution(&execution_block("off")))
            .expect("parses");
    let block = disarmed
        .execution
        .expect("a disarmed block is still a block");
    assert_eq!(block.mode, ExecutionMode::Off);
    assert!(
        block.min_base_balance > 0.0 && block.min_quote_balance > 0.0,
        "the limits are validated in every mode, so arming is one word and never a late failure"
    );
}

/// The two floors are asymmetric on purpose. The quote floor reserves cash the engine must not
/// spend down, so zero is a decision to trade the account to nothing and is refused. The base floor
/// reserves the asset being SOLD, and where the base asset IS the position — outcome shares — zero
/// is the only correct value: any positive reserve subtracts from every exit, so a full-position
/// offer reads Underfunded and a flatten rounds under the venue's minimum size and strands the
/// position to resolution. Negative is nonsense on either. Each refusal names the field.
#[test]
fn the_base_floor_may_be_zero_and_the_quote_floor_may_not() {
    let zero_base = config_with_execution(&execution_block_overriding(
        "live",
        "min_base_balance",
        "0.0",
    ));
    let config = polysim::config::Config::<NoParams>::from_yaml(&zero_base)
        .expect("zero is the only correct base reserve where the base asset IS the position");
    assert_eq!(
        config.execution.expect("execution block").min_base_balance,
        0.0
    );

    for (field, value) in [
        ("min_base_balance", "-0.001"),
        ("min_quote_balance", "0.0"),
        ("min_quote_balance", "-1.0"),
    ] {
        let yaml = config_with_execution(&execution_block_overriding("live", field, value));
        let error = polysim::config::Config::<NoParams>::from_yaml(&yaml)
            .expect_err("a reserve below zero, and a quote reserve of zero, must be refused");
        let text = error.to_string();
        assert!(
            text.contains(field),
            "the refusal names the field an operator edits, got {text}"
        );
    }
}

/// Zero is the trap the other limits set: everywhere else in this block it means "no ceiling", and
/// an operator writing it here would be reaching for exactly that. The reject counter records the
/// event before the engine compares it, so zero halts on the FIRST hard reject — the tightest
/// setting available, wearing the look of the loosest. One is the real minimum and stays legal.
#[test]
fn a_zero_reject_ceiling_is_refused_rather_than_read_as_no_ceiling() {
    let error = refusal(&config_with_execution(&execution_block_with(
        "live",
        "  max_consecutive_rejects: 0\n",
    )));
    assert!(
        matches!(
            &error,
            ConfigError::Invalid { field, .. } if *field == "execution.max_consecutive_rejects"
        ),
        "the refusal names the field an operator edits, got {error}"
    );

    let one = config_with_execution(&execution_block_with(
        "live",
        "  max_consecutive_rejects: 1\n",
    ));
    let config = polysim::config::Config::<NoParams>::from_yaml(&one)
        .expect("halting on the first hard reject is a deliberate setting, spelled 1");
    assert_eq!(
        config
            .execution
            .expect("execution block")
            .max_consecutive_rejects,
        1
    );
}

#[test]
fn the_quote_distance_safety_band_cannot_be_disabled_by_overflow() {
    let yaml = config_with_execution(&execution_block_overriding(
        "live",
        "max_quote_distance_bps",
        "1e308",
    ));
    let error = refusal(&yaml);
    assert!(
        matches!(
            &error,
            ConfigError::Invalid {
                field: "execution.max_quote_distance_bps",
                ..
            }
        ),
        "an enormous finite float must not saturate the runtime centi-bps conversion, got {error}"
    );
}

#[test]
fn execution_durations_must_fit_the_runtime_microsecond_type() {
    let max = u64::MAX.to_string();
    let cases = [
        (
            "execution.max_book_age_ms",
            execution_block_overriding("live", "max_book_age_ms", &max),
        ),
        (
            "execution.inflight_timeout_ms",
            format!("{}  inflight_timeout_ms: {max}\n", execution_block("live")),
        ),
        (
            "execution.order_reap_secs",
            format!("{}  order_reap_secs: {max}\n", execution_block("live")),
        ),
        (
            "execution.disconnect_sweep_secs",
            format!(
                "{}  disconnect_sweep_secs: {max}\n",
                execution_block("live")
            ),
        ),
        (
            "execution.max_clock_skew_ms",
            format!("{}  max_clock_skew_ms: {max}\n", execution_block("live")),
        ),
    ];
    for (expected_field, block) in cases {
        let error = refusal(&config_with_execution(&block));
        assert!(
            matches!(
                &error,
                ConfigError::Invalid { field, value, .. }
                    if *field == expected_field && value.as_ref() == max
            ),
            "{expected_field} overflow is refused by field and value, got {error}"
        );
    }
}

#[test]
fn simulated_durations_and_combined_retention_must_fit_microseconds() {
    let max = u64::MAX.to_string();
    for (field, baseline) in [
        ("order_entry_latency_ms", "15"),
        ("ack_latency_ms", "15"),
        ("max_market_data_delay_ms", "1000"),
        ("heartbeat_ms", "100"),
    ] {
        let sim = SIM_BLOCK.replace(&format!("{field}: {baseline}"), &format!("{field}: {max}"));
        let error = refusal(&config_with_execution_on(
            BINANCE_SPOT_SOURCE,
            &execution_block_with("sim", &sim),
        ));
        let expected_field = format!("execution.sim.{field}");
        assert!(
            matches!(
                &error,
                ConfigError::Invalid { field, value, .. }
                    if *field == expected_field && value.as_ref() == max
            ),
            "{expected_field} overflow is refused by field and value, got {error}"
        );
    }

    let block = format!(
        "{}{}  order_reap_secs: 86400\n",
        execution_block("sim"),
        SIM_BLOCK
    );
    let error = refusal(&config_with_execution_on(BINANCE_SPOT_SOURCE, &block));
    assert!(
        matches!(
            error,
            ConfigError::Invalid {
                field: "execution.sim",
                ..
            }
        ),
        "the separately valid reap and timeout spans must also stay inside the operational ceiling, got {error}"
    );
}

#[test]
fn execution_depth_must_fit_the_fixed_eight_level_ladder() {
    for value in ["0", "9"] {
        let block = format!("{}  max_orders_per_side: {value}\n", execution_block("off"));
        let yaml = config_with_execution(&block);
        let error = polysim::config::Config::<NoParams>::from_yaml(&yaml)
            .expect_err("depth outside 1..=8 must be refused");
        let text = error.to_string();
        assert!(text.contains("max_orders_per_side"), "got {text}");
        assert!(text.contains("1..=8"), "got {text}");
    }

    let yaml = config_with_execution(&format!(
        "{}  max_orders_per_side: 8\n",
        execution_block("off")
    ));
    let config = polysim::config::Config::<NoParams>::from_yaml(&yaml)
        .expect("the full fixed ladder is valid");
    assert_eq!(
        config
            .execution
            .expect("execution block")
            .max_orders_per_side,
        8
    );
}

/// A mode a venue has no edge for would come up reporting itself ARMED while placing nothing —
/// the one way "configured to trade" and "trading" can disagree without anyone noticing — so the
/// capability table is enforced at parse rather than warned about at wiring, and the queue follows
/// it. A DISARMED block is fine on any source: it states limits, not intent to send.
#[test]
fn a_venue_is_armed_only_for_the_execution_modes_it_has_an_edge_for() {
    for (spelling, expects_queue) in [("live", true), ("off", false)] {
        let yaml = config_with_execution_on(POLY_SOURCE, &execution_block(spelling));
        let config: polysim::config::Config = polysim::config::Config::from_yaml(&yaml)
            .expect("polymarket has both an off and a live execution edge");
        let registry = Registry::build(&config).expect("registry builds");
        assert_eq!(
            registry.exec_queue_id().is_some(),
            expects_queue,
            "{spelling}: the exec input queue exists exactly when an edge will feed it"
        );
        assert_eq!(
            registry.input_queue_count(),
            if expects_queue { 3 } else { 2 },
            "{spelling}: one producer group for the four slots + the timer, plus execution when armed"
        );
    }
}
