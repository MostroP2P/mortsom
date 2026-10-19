//! UI automation boundary consumed by adapters and the coordinator. The
//! real implementations are the Linux and Web sessions; tests inject fakes.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::UiError;

/// A UI automation session bound to one device and one app.
#[async_trait]
pub trait UiSession: Send + Sync {
    /// Whether an element with this accessibility id exists now.
    async fn exists(&self, accessibility_id: &str) -> Result<bool, UiError>;
    /// Whether the element is on the screen or reachable by scrolling to
    /// it. Ask this about a list; it may swipe the screen to answer.
    /// Defaults to [`Self::exists`] for sessions with nothing to scroll.
    async fn exists_anywhere(&self, accessibility_id: &str) -> Result<bool, UiError> {
        self.exists(accessibility_id).await
    }
    /// Waits for an element to appear.
    async fn wait_for(&self, accessibility_id: &str, timeout: Duration) -> Result<(), UiError>;
    /// Waits for an element to disappear.
    async fn wait_gone(&self, accessibility_id: &str, timeout: Duration) -> Result<(), UiError>;
    /// Taps an element.
    async fn tap(&self, accessibility_id: &str) -> Result<(), UiError>;
    /// Clears and types into an element. Text is never logged.
    async fn type_text(&self, accessibility_id: &str, text: &str) -> Result<(), UiError>;
    /// Visible text of an element.
    async fn text_of(&self, accessibility_id: &str) -> Result<String, UiError>;
    /// Whether an element is enabled.
    async fn is_enabled(&self, accessibility_id: &str) -> Result<bool, UiError>;
    /// Accessibility label (state readout) of an element.
    async fn label_of(&self, accessibility_id: &str) -> Result<String, UiError>;
    /// PNG screenshot.
    async fn screenshot_png(&self) -> Result<Vec<u8>, UiError>;
    /// Accessibility hierarchy XML.
    async fn page_source(&self) -> Result<String, UiError>;
    /// Background for `secs` seconds and return.
    async fn background(&self, secs: u64) -> Result<(), UiError>;
    /// Force-stop the app.
    async fn terminate_app(&self) -> Result<(), UiError>;
    /// Bring the app to the foreground, launching if needed.
    async fn activate_app(&self) -> Result<(), UiError>;
    /// Clear app data.
    async fn clear_app(&self) -> Result<(), UiError>;
    /// Enable or disable network connectivity (Wi-Fi and mobile data).
    async fn set_connectivity(&self, enabled: bool) -> Result<(), UiError>;
    /// Rotate the device to landscape or portrait.
    async fn set_orientation(&self, landscape: bool) -> Result<(), UiError>;
    /// Dismiss the soft keyboard if one is up.
    async fn hide_keyboard(&self) -> Result<(), UiError>;
    /// Text logs the session's own processes wrote so far (the app's
    /// stdout, a driver log), as `(file name, contents)` pairs, for the run
    /// to keep as evidence before the session and its private directories
    /// go. Empty for a session that keeps none.
    async fn diagnostics(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// End the session.
    async fn end(&self) -> Result<(), UiError>;
}

/// Scripted UI session for adapter tests in this and other crates.
#[cfg(any(test, feature = "testing"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod tests_support {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;

    use super::UiSession;
    use crate::error::UiError;

    /// Scripted UI: elements exist when listed (dynamic changes via `on_tap`),
    /// labels answer from a map, and every action is recorded.
    #[derive(Default)]
    pub struct ScriptedUi {
        present: Mutex<Vec<String>>,
        labels: Mutex<BTreeMap<String, String>>,
        text: Mutex<BTreeMap<String, mortsom_common::Secret>>,
        ignored_text: Mutex<Vec<String>>,
        /// Controls whose next `n` typed values the app throws away after
        /// accepting them: a form rebuilt by an incoming message.
        wiped_text: Mutex<BTreeMap<String, u32>>,
        calls: Mutex<Vec<String>>,
        on_tap: Mutex<BTreeMap<String, Vec<String>>>,
        /// Elements that disappear after `id` is tapped (navigation away).
        on_tap_hide: Mutex<BTreeMap<String, Vec<String>>>,
        /// Label changes triggered by a tap.
        on_tap_relabel: Mutex<BTreeMap<String, (String, String)>>,
        /// Labels still settling: `(interim value, reads left)` before the
        /// stored label is what a read returns.
        label_pending: Mutex<BTreeMap<String, (String, u32)>>,
        /// Elements rendered below the fold: only a scrolling look finds them.
        below: Mutex<Vec<String>>,
        /// Elements the app has not drawn yet, with the number of probes
        /// still to miss them.
        pending: Mutex<BTreeMap<String, u32>>,
        /// Elements the soft keyboard covers while it is up.
        under_keyboard: Mutex<Vec<String>>,
        /// Whether typing has raised the keyboard.
        keyboard_up: Mutex<bool>,
        /// Taps that land while the app settles and do nothing, per id.
        swallowed: Mutex<BTreeMap<String, u32>>,
        /// Controls the app has greyed out while it works.
        disabled: Mutex<Vec<String>>,
        /// Controls that grey out once tapped, the way a submit does while
        /// the app is still working on the request.
        disabled_after_tap: Mutex<Vec<String>>,
        /// Controls a tap makes pressable: a confirm that waits for a pick.
        on_tap_enable: Mutex<BTreeMap<String, Vec<String>>>,
        /// Elements a tap puts on screen a number of looks later.
        on_tap_draws_after: Mutex<BTreeMap<String, (Vec<String>, u32)>>,
        /// Elements that leave the screen after answering this many more
        /// probes; reading one that has left fails.
        vanishing: Mutex<BTreeMap<String, u32>>,
        end_failures: Mutex<u32>,
        /// How long each screenshot takes.
        screenshot_delay: Mutex<Duration>,
    }

    impl ScriptedUi {
        /// Each screenshot takes `delay`, as a large window's does.
        pub fn slow_screenshots(&self, delay: Duration) {
            *self.screenshot_delay.lock().unwrap() = delay;
        }
        /// The next `times` values typed into `id` are accepted and then lost,
        /// as when the app rebuilds a form on a message that arrives mid-entry.
        pub fn wipes_text(&self, id: &str, times: u32) {
            self.wiped_text.lock().unwrap().insert(id.into(), times);
        }
        /// Acknowledges edits without accepting their values for this control.
        pub fn ignore_text(&self, id: &str) {
            self.ignored_text.lock().unwrap().push(id.into());
        }
        /// Injects transient shutdown failures while retaining this fake session.
        pub fn fail_end_times(&self, failures: u32) {
            *self.end_failures.lock().unwrap() = failures;
        }
        /// Marks `ids` as present.
        pub fn present(&self, ids: &[&str]) {
            self.present
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }
        /// Sets the accessibility label answered for `id`.
        pub fn label(&self, id: &str, value: &str) {
            self.labels.lock().unwrap().insert(id.into(), value.into());
        }
        /// The first `reads` reads of `id` return `interim`; a screen in
        /// transition shows a stale or empty readout before the real one.
        pub fn label_settles(&self, id: &str, interim: &str, reads: u32) {
            self.label_pending
                .lock()
                .unwrap()
                .insert(id.into(), (interim.into(), reads));
        }
        /// Elements that appear after `id` is tapped.
        pub fn on_tap(&self, id: &str, appear: &[&str]) {
            self.on_tap
                .lock()
                .unwrap()
                .insert(id.into(), appear.iter().map(|s| (*s).to_owned()).collect());
        }
        /// After `id` is tapped, `target`'s label becomes `value`.
        pub fn on_tap_relabel(&self, id: &str, target: &str, value: &str) {
            self.on_tap_relabel
                .lock()
                .unwrap()
                .insert(id.into(), (target.into(), value.into()));
        }

        /// Elements that disappear after `id` is tapped.
        pub fn on_tap_hide(&self, id: &str, hide: &[&str]) {
            self.on_tap_hide
                .lock()
                .unwrap()
                .insert(id.into(), hide.iter().map(|s| (*s).to_owned()).collect());
        }
        /// Every action performed, in order.
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        /// Whether `id` is present now.
        pub fn has(&self, id: &str) -> bool {
            self.present.lock().unwrap().iter().any(|p| p == id)
        }

        /// Marks `ids` as rendered but off-screen: a plain probe does not
        /// see them, while a wait and a list probe both scroll and do.
        pub fn below_the_fold(&self, ids: &[&str]) {
            self.below
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }

        /// Makes `id` miss its next `probes` looks before it is there.
        ///
        /// A cold start is exactly this: the session opens before the app
        /// has drawn its first frame, so an element the screen will show is
        /// briefly absent. A double that is either present or not cannot
        /// tell a screen apart from a screen that has not arrived.
        pub fn appears_after(&self, id: &str, probes: u32) {
            self.pending.lock().unwrap().insert(id.into(), probes);
        }

        /// Marks `ids` as the part of the screen the soft keyboard covers.
        ///
        /// Typing raises it and `hide_keyboard` puts it down. A covered
        /// element is gone, not merely off-screen: the app's semantics stop
        /// at the shrunken viewport, so no amount of scrolling finds it --
        /// which is why a double that only knows "above" and "below the
        /// fold" cannot express this.
        pub fn keyboard_over(&self, ids: &[&str]) {
            self.under_keyboard
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }

        /// Makes the next `taps` on `id` land and do nothing.
        ///
        /// An app rebuilding a screen -- after a relaunch, or as a keyboard
        /// closes under the button -- takes the tap and acts on none of it.
        /// The control is there and the driver hits it, so a double that
        /// always acts on a tap cannot express this.
        pub fn swallows_taps(&self, id: &str, taps: u32) {
            self.swallowed.lock().unwrap().insert(id.into(), taps);
        }

        /// `id` answers `probes` more presence probes and then leaves the
        /// screen, and a read of it fails from then on: a readout the app
        /// withdraws between the look that found it and the read of it.
        pub fn vanishes_after(&self, id: &str, probes: u32) {
            self.vanishing.lock().unwrap().insert(id.into(), probes);
        }

        /// Counts one probe of a vanishing `id`, removing it on the last.
        fn probe_vanishing(&self, id: &str) {
            let mut vanishing = self.vanishing.lock().unwrap();
            if let Some(left) = vanishing.get_mut(id) {
                *left = left.saturating_sub(1);
                if *left == 0 {
                    self.present.lock().unwrap().retain(|p| p != id);
                }
            }
        }

        /// `id` accepts one press and then greys out: the app took the
        /// request and is still working on it.
        pub fn disables_after_tap(&self, id: &str) {
            self.disabled_after_tap.lock().unwrap().push(id.to_owned());
        }

        /// Tapping `id` puts `appear` on screen, each missing its next
        /// `probes` looks: a route the tap pops back to, drawn a few frames
        /// after the one it closes has gone.
        pub fn on_tap_draws_after(&self, id: &str, appear: &[&str], probes: u32) {
            let appear = appear.iter().map(|s| (*s).to_owned()).collect();
            self.on_tap_draws_after
                .lock()
                .unwrap()
                .insert(id.into(), (appear, probes));
        }

        /// Tapping `id` makes `enable` pressable again.
        pub fn on_tap_enable(&self, id: &str, enable: &[&str]) {
            self.on_tap_enable
                .lock()
                .unwrap()
                .insert(id.into(), enable.iter().map(|s| (*s).to_owned()).collect());
        }

        /// Greys `ids` out: on screen, but not accepting a press.
        ///
        /// A form that has been submitted and is waiting on the network
        /// looks like this -- the button is still drawn, and pressing it
        /// again does nothing the app would honour. A double that answers
        /// every control as enabled cannot tell that apart from a form
        /// that was never submitted at all.
        pub fn disables(&self, ids: &[&str]) {
            self.disabled
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }

        /// Whether this tap is swallowed, counting it if so.
        fn swallows(&self, id: &str) -> bool {
            let mut swallowed = self.swallowed.lock().unwrap();
            match swallowed.get_mut(id) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    true
                }
                _ => false,
            }
        }

        /// Whether the keyboard is covering `id` right now.
        fn covered(&self, id: &str) -> bool {
            *self.keyboard_up.lock().unwrap()
                && self.under_keyboard.lock().unwrap().iter().any(|p| p == id)
        }

        /// Whether `id` is anywhere on the current screen, fold or no fold.
        pub fn has_anywhere(&self, id: &str) -> bool {
            self.has(id) || self.below.lock().unwrap().iter().any(|p| p == id)
        }

        /// One probe against a possibly still-drawing element: consumes a
        /// pending frame and reports whether the element is up yet.
        fn drawn(&self, id: &str) -> bool {
            let mut pending = self.pending.lock().unwrap();
            match pending.get_mut(id) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    false
                }
                Some(_) => {
                    pending.remove(id);
                    true
                }
                None => true,
            }
        }
    }

    #[async_trait]
    impl UiSession for ScriptedUi {
        async fn exists(&self, id: &str) -> Result<bool, UiError> {
            let there = self.drawn(id) && self.has(id) && !self.covered(id);
            self.probe_vanishing(id);
            Ok(there)
        }
        async fn exists_anywhere(&self, id: &str) -> Result<bool, UiError> {
            // An element the screen has not drawn yet is absent below the
            // fold as much as above it: scrolling cannot find it either.
            Ok(self.drawn(id) && self.has_anywhere(id) && !self.covered(id))
        }
        async fn wait_for(&self, id: &str, _: Duration) -> Result<(), UiError> {
            // A wait scrolls before it gives up, so it sees below the fold --
            // but not under the keyboard, which is not a scrolling problem.
            if self.has_anywhere(id) && !self.covered(id) {
                Ok(())
            } else {
                Err(UiError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "not found",
                    timeout: Duration::ZERO,
                })
            }
        }
        async fn wait_gone(&self, id: &str, _: Duration) -> Result<(), UiError> {
            if self.has(id) {
                Err(UiError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "still present",
                    timeout: Duration::ZERO,
                })
            } else {
                Ok(())
            }
        }
        async fn tap(&self, id: &str) -> Result<(), UiError> {
            // A control the keyboard covers cannot be tapped: it is not on
            // the screen to be hit. A double that let it through would pass
            // a flow the real driver cannot perform.
            if self.covered(id) {
                return Err(UiError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "under the keyboard",
                    timeout: Duration::ZERO,
                });
            }
            self.calls.lock().unwrap().push(format!("tap {id}"));
            if self.swallows(id) {
                return Ok(());
            }
            if self
                .disabled_after_tap
                .lock()
                .unwrap()
                .iter()
                .any(|d| d == id)
            {
                self.disabled.lock().unwrap().push(id.to_owned());
            }
            if let Some(enable) = self.on_tap_enable.lock().unwrap().get(id).cloned() {
                self.disabled
                    .lock()
                    .unwrap()
                    .retain(|d| !enable.contains(d));
            }
            if let Some(appear) = self.on_tap.lock().unwrap().get(id).cloned() {
                self.present.lock().unwrap().extend(appear);
            }
            if let Some((appear, probes)) = self.on_tap_draws_after.lock().unwrap().get(id).cloned()
            {
                for target in &appear {
                    self.pending.lock().unwrap().insert(target.clone(), probes);
                }
                self.present.lock().unwrap().extend(appear);
            }
            if let Some(hide) = self.on_tap_hide.lock().unwrap().get(id).cloned() {
                self.present.lock().unwrap().retain(|p| !hide.contains(p));
            }
            if let Some((target, value)) = self.on_tap_relabel.lock().unwrap().get(id).cloned() {
                self.labels.lock().unwrap().insert(target, value);
            }
            Ok(())
        }
        async fn hide_keyboard(&self) -> Result<(), UiError> {
            self.calls.lock().unwrap().push("hide keyboard".into());
            *self.keyboard_up.lock().unwrap() = false;
            Ok(())
        }
        async fn type_text(&self, id: &str, text: &str) -> Result<(), UiError> {
            *self.keyboard_up.lock().unwrap() = true;
            // Record the length only: secrets never reach logs, not even fakes'.
            self.calls
                .lock()
                .unwrap()
                .push(format!("type {id} ({} chars)", text.len()));
            let wiped = {
                let mut wiped = self.wiped_text.lock().unwrap();
                match wiped.get_mut(id) {
                    Some(left) if *left > 0 => {
                        *left -= 1;
                        true
                    }
                    _ => false,
                }
            };
            if wiped {
                self.text.lock().unwrap().remove(id);
            } else if !self
                .ignored_text
                .lock()
                .unwrap()
                .iter()
                .any(|ignored| ignored == id)
            {
                self.text
                    .lock()
                    .unwrap()
                    .insert(id.into(), mortsom_common::Secret::new(text));
            }
            Ok(())
        }
        async fn text_of(&self, id: &str) -> Result<String, UiError> {
            Ok(self
                .text
                .lock()
                .unwrap()
                .get(id)
                .map(|value| value.expose().to_owned())
                .unwrap_or_default())
        }
        async fn is_enabled(&self, id: &str) -> Result<bool, UiError> {
            Ok(!self.disabled.lock().unwrap().iter().any(|d| d == id))
        }
        async fn label_of(&self, id: &str) -> Result<String, UiError> {
            if self.vanishing.lock().unwrap().contains_key(id) && !self.has(id) {
                return Err(UiError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "not found",
                    timeout: Duration::ZERO,
                });
            }
            {
                let mut pending = self.label_pending.lock().unwrap();
                if let Some((interim, left)) = pending.get_mut(id) {
                    if *left > 0 {
                        *left -= 1;
                        return Ok(interim.clone());
                    }
                    pending.remove(id);
                }
            }
            Ok(self
                .labels
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .unwrap_or_default())
        }
        async fn screenshot_png(&self) -> Result<Vec<u8>, UiError> {
            let delay = *self.screenshot_delay.lock().unwrap();
            tokio::time::sleep(delay).await;
            Ok(vec![])
        }
        async fn page_source(&self) -> Result<String, UiError> {
            // Hierarchy dump listing every present id as a resource-id; an
            // element still drawing is not in it yet, and each dump is one
            // probe of it.
            let present = self.present.lock().unwrap().clone();
            Ok(present
                .iter()
                .filter(|id| self.drawn(id))
                .map(|id| format!("<node resource-id=\"{id}\"/>"))
                .collect())
        }
        async fn background(&self, _: u64) -> Result<(), UiError> {
            Ok(())
        }
        async fn terminate_app(&self) -> Result<(), UiError> {
            Ok(())
        }
        async fn activate_app(&self) -> Result<(), UiError> {
            Ok(())
        }
        async fn clear_app(&self) -> Result<(), UiError> {
            Ok(())
        }
        async fn set_connectivity(&self, _: bool) -> Result<(), UiError> {
            Ok(())
        }
        async fn set_orientation(&self, _: bool) -> Result<(), UiError> {
            Ok(())
        }
        async fn end(&self) -> Result<(), UiError> {
            let mut failures = self.end_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                self.calls.lock().unwrap().push("end failed".into());
                return Err(UiError::WebDriver(
                    crate::webdriver::WebDriverError::Transport("injected cleanup failure".into()),
                ));
            }
            self.calls.lock().unwrap().push("end session".into());
            Ok(())
        }
    }
}
