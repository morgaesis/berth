pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn build_id() -> &'static str {
    env!("BERTH_BUILD_ID")
}

pub fn build_target() -> &'static str {
    env!("BERTH_BUILD_TARGET")
}

pub fn long_version() -> String {
    format!("{} ({})", version(), build_id())
}
