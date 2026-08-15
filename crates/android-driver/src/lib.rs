//! Mortsom Android driver: W3C WebDriver client for Appium/UiAutomator2 plus
//! AVD, emulator and adb primitives.

pub mod avd;
pub mod config;
pub mod process;
pub mod sdk;

pub use avd::{Actor, AvdSpec};
pub use config::AndroidConfig;
pub use process::{Command, CommandRunner, TokioRunner};
pub use sdk::AndroidSdk;
