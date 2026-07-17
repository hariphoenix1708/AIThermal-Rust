pub mod android_prop;
pub mod peridot;
pub mod probe;
pub mod profile;
pub mod report;

#[allow(unused_imports)]
pub use profile::HardwareProfile;

pub mod kernel;
pub mod memory;
#[allow(dead_code)]
pub mod network;
pub mod scheduler;
pub mod services;
pub mod storage;

pub mod discovery;

pub mod capability;
pub mod charging;
pub mod cpu;
pub mod cpuset;
#[allow(dead_code)]
pub mod display;
pub mod gpu;
pub mod thermal;
pub mod screen_netlink;
