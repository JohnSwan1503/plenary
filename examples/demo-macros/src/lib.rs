//! The example bundle. `stats` is listed first on purpose: its handler reads
//! facts the other two produce, so if ordering came from list order this
//! bundle would fail. It works because ordering comes from signatures (R6).
plenary_plugin::bundle!(stats_plugin::plugin, registry_plugin::plugin, stq_lite_plugin::plugin);
