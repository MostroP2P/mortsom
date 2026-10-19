//! Mortsom UI driver foundation: the `UiSession` boundary every client
//! implements, the process runner the coordinator shells out with, and the
//! thin W3C WebDriver client behind the Web session.

pub mod error;
pub mod process;
pub mod timed;
pub mod ui;
pub mod webdriver;

pub use error::UiError;
pub use process::{Command, CommandRunner, TokioRunner};
pub use ui::UiSession;
pub use webdriver::{Element, Locator, Session, WebDriverError};
