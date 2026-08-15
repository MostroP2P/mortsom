//! Mortsom Android driver: W3C WebDriver client for Appium/UiAutomator2 plus
//! AVD, emulator and adb primitives.

pub mod adb;
pub mod appium;
pub mod avd;
pub mod config;
pub mod emulator;
pub mod process;
pub mod sdk;
pub mod webdriver;

pub use adb::Adb;
pub use appium::{AppiumError, AppiumServer, AppiumSession, SessionSpec};
pub use avd::{Actor, AvdSpec};
pub use config::AndroidConfig;
pub use emulator::{DeviceSlot, Emulator, LaunchOptions};
pub use process::{Command, CommandRunner, TokioRunner};
pub use sdk::AndroidSdk;
pub use webdriver::{Element, Locator, Session, WebDriverError};
