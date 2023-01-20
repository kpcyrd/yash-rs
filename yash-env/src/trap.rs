// This file is part of yash, an extended POSIX shell.
// Copyright (C) 2021 WATANABE Yuki
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Signal and other event handling settings.
//!
//! The trap is a mechanism of the shell that allows you to configure event
//! handlers for specific situations. A [`TrapSet`] is a mapping from [`Condition`]s to
//! [`Action`]s. When the mapping is modified, it updates the corresponding signal
//! disposition in the underlying system through a [`SignalSystem`] implementor.
//! Methods of `TrapSet` expect they are passed the same system instance in
//! every call to keep it in a correct state.
//!
//! `TrapSet` manages two types of signal handling configurations. One is
//! user-defined traps, which the user explicitly configures with the trap
//! built-in. The other is internal handlers, which the shell implicitly
//! installs to the system to implement additional actions it needs to perform.
//! `TrapSet` merges the two configurations into a single [`SignalHandling`] for
//! each signal and sets it to the system.
//!
//! No signal handling is involved for conditions other than signals, and the
//! trap set serves only as a storage for action settings.

use crate::system::{Errno, SignalHandling};
#[cfg(doc)]
use crate::system::{SharedSystem, System};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::rc::Rc;
use yash_syntax::source::Location;

#[doc(no_inline)]
pub use nix::sys::signal::Signal;

/// System interface for signal handling configuration.
pub trait SignalSystem {
    /// Sets how a signal is handled.
    ///
    /// This function updates the signal blocking mask and the signal action for
    /// the specified signal, and returns the previous action.
    fn set_signal_handling(
        &mut self,
        signal: Signal,
        handling: SignalHandling,
    ) -> Result<SignalHandling, Errno>;
}

/// Condition under which an [`Action`] is executed
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Condition {
    /// When the shell exits
    Exit,
    /// When the specified signal is delivered to the shell process
    Signal(Signal),
}

/// Conversion from `Condition` to `String`
///
/// The result is an uppercase string representing the condition such as
/// `"EXIT"` and `"TERM"`.
impl std::fmt::Display for Condition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Condition::Exit => "EXIT".fmt(f),
            Condition::Signal(signal) => {
                let full_name = signal.as_str();
                let name = full_name.strip_prefix("SIG").unwrap_or(full_name);
                name.fmt(f)
            }
        }
    }
}

/// Error in conversion from string to [`Condition`]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ParseConditionError;

/// Conversion from `String` to `Condition`
///
/// This implementation supports parsing uppercase strings like `"EXIT"` and
/// `"TERM"`.
impl std::str::FromStr for Condition {
    type Err = ParseConditionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // TODO Make case-insensitive
        // TODO Allow SIG-prefix
        match s {
            "EXIT" => Ok(Self::Exit),
            _ => match format!("SIG{s}").parse() {
                Ok(signal) => Ok(Self::Signal(signal)),
                Err(_) => Err(ParseConditionError),
            },
        }
    }
}

/// Action performed when a [`Condition`] is met
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Performs the default action.
    ///
    /// For signal conditions, the behavior depends on the signal delivered.
    /// For other conditions, this is equivalent to `Ignore`.
    Default,

    /// Pretends as if the condition was not met.
    Ignore,

    /// Executes a command string.
    Command(Rc<str>),
}

impl Default for Action {
    fn default() -> Self {
        Action::Default
    }
}

impl From<&Action> for SignalHandling {
    fn from(trap: &Action) -> Self {
        match trap {
            Action::Default => SignalHandling::Default,
            Action::Ignore => SignalHandling::Ignore,
            Action::Command(_) => SignalHandling::Catch,
        }
    }
}

/// Error that may happen in [`TrapSet::set_action`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetActionError {
    /// Attempt to set a trap that has been ignored since the shell startup.
    InitiallyIgnored,
    /// Attempt to set a trap for the `SIGKILL` signal.
    SIGKILL,
    /// Attempt to set a trap for the `SIGSTOP` signal.
    SIGSTOP,
    /// Error from the underlying system interface.
    SystemError(Errno),
}

impl std::fmt::Display for SetActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use SetActionError::*;
        match self {
            InitiallyIgnored => "the signal has been ignored since startup".fmt(f),
            SIGKILL => "cannot set a trap for SIGKILL".fmt(f),
            SIGSTOP => "cannot set a trap for SIGSTOP".fmt(f),
            SystemError(errno) => errno.fmt(f),
        }
    }
}

impl std::error::Error for SetActionError {}

impl From<Errno> for SetActionError {
    fn from(errno: Errno) -> Self {
        SetActionError::SystemError(errno)
    }
}

/// State of the trap action for a condition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrapState {
    /// Action taken when the condition is met.
    pub action: Action,
    /// Location of the simple command that invoked the trap built-in that set
    /// the current action.
    pub origin: Location,
    /// True iff a signal specified by the condition has been caught and the
    /// action command has not yet executed.
    pub pending: bool,
}

/// User-visible trap setting.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Setting {
    /// The user has not yet set a trap for the signal specified by the
    /// condition, and the signal disposition the shell has inherited from the
    /// pre-exec process is `SIG_DFL`.
    InitiallyDefaulted,
    /// The user has not yet set a trap for the signal specified by the
    /// condition, and the signal disposition the shell has inherited from the
    /// pre-exec process is `SIG_IGN`.
    InitiallyIgnored,
    /// User-defined trap.
    UserSpecified(TrapState),
}

impl Setting {
    fn as_trap(&self) -> Option<&TrapState> {
        if let Setting::UserSpecified(trap) = self {
            Some(trap)
        } else {
            None
        }
    }
}

impl From<&Setting> for SignalHandling {
    fn from(state: &Setting) -> Self {
        match state {
            Setting::InitiallyDefaulted => SignalHandling::Default,
            Setting::InitiallyIgnored => SignalHandling::Ignore,
            Setting::UserSpecified(trap) => (&trap.action).into(),
        }
    }
}

/// Whole configuration and state for a trap condition.
#[derive(Clone, Debug)]
struct GrandState {
    /// Setting that is effective in the current environment.
    current_setting: Setting,

    /// Setting that was effective in the parent environment.
    parent_setting: Option<Setting>,

    /// Whether the internal handler has been installed in the current environment.
    internal_handler_enabled: bool,
}

/// Iterator of trap actions configured in a [trap set](TrapSet).
///
/// [`TrapSet::iter`] returns this type of iterator.
#[must_use]
pub struct Iter<'a> {
    inner: std::collections::btree_map::Iter<'a, Condition, GrandState>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a Signal, Option<&'a TrapState>, Option<&'a TrapState>);
    fn next(&mut self) -> Option<(&'a Signal, Option<&'a TrapState>, Option<&'a TrapState>)> {
        loop {
            let (cond, state) = self.inner.next()?;
            let current = &state.current_setting;
            let current = current.as_trap();
            let parent = &state.parent_setting;
            let parent = parent.as_ref().and_then(Setting::as_trap);
            if let Condition::Signal(signal) = cond {
                if current.is_some() || parent.is_some() {
                    return Some((signal, current, parent));
                }
            }
        }
    }
}

/// Collection of event handling settings.
///
/// See the [module documentation](self) for details.
#[derive(Clone, Debug, Default)]
pub struct TrapSet {
    traps: BTreeMap<Condition, GrandState>,
}

// TODO Extend internal handlers for other signals
impl TrapSet {
    /// Returns the current state for a signal.
    ///
    /// This function returns a pair of optional trap states. The first is the
    /// currently configured trap action, and the second is the action set
    /// before [`enter_subshell`](Self::enter_subshell) was called.
    ///
    /// This function does not reflect the initial signal actions the shell
    /// inherited on startup.
    pub fn get_state(&self, signal: Signal) -> (Option<&TrapState>, Option<&TrapState>) {
        match self.traps.get(&Condition::Signal(signal)) {
            None => (None, None),
            Some(state) => {
                let current = &state.current_setting;
                let current = current.as_trap();
                let parent = &state.parent_setting;
                let parent = parent.as_ref().and_then(Setting::as_trap);
                (current, parent)
            }
        }
    }

    /// Sets a trap action for a signal.
    ///
    /// This function installs a signal handler to the specified underlying
    /// system.
    ///
    /// If `override_ignore` is `false`, you cannot set a trap for a signal that
    /// has been ignored since the shell startup. An interactive shell should
    /// set `override_ignore` to `true` to bypass this restriction.
    ///
    /// You can never set a trap for `SIGKILL` or `SIGSTOP`.
    ///
    /// `origin` should be the location of the command performing this trap
    /// update. It is only informative: It does not affect the signal handling
    /// behavior and can be referenced later by [`get_state`](Self::get_state).
    ///
    /// This function clears all parent states remembered when [entering a
    /// subshell](Self::enter_subshell), not only for `signal` but also for
    /// other signals.
    pub fn set_action<S: SignalSystem>(
        &mut self,
        system: &mut S,
        signal: Signal,
        action: Action,
        origin: Location,
        override_ignore: bool,
    ) -> Result<(), SetActionError> {
        match signal {
            Signal::SIGKILL => return Err(SetActionError::SIGKILL),
            Signal::SIGSTOP => return Err(SetActionError::SIGSTOP),
            _ => (),
        }

        self.clear_parent_settings();

        let state = TrapState {
            action,
            origin,
            pending: false,
        };

        let entry = match self.traps.entry(Condition::Signal(signal)) {
            Entry::Vacant(vacant) => {
                if !override_ignore {
                    let initial_handling =
                        system.set_signal_handling(signal, SignalHandling::Ignore)?;
                    if initial_handling == SignalHandling::Ignore {
                        vacant.insert(GrandState {
                            current_setting: Setting::InitiallyIgnored,
                            parent_setting: None,
                            internal_handler_enabled: false,
                        });
                        return Err(SetActionError::InitiallyIgnored);
                    }
                }
                Entry::Vacant(vacant)
            }
            Entry::Occupied(mut occupied) => {
                if !override_ignore && occupied.get().current_setting == Setting::InitiallyIgnored {
                    return Err(SetActionError::InitiallyIgnored);
                }
                if occupied.get().internal_handler_enabled {
                    occupied.get_mut().current_setting = Setting::UserSpecified(state);
                    return Ok(());
                }
                Entry::Occupied(occupied)
            }
        };

        system.set_signal_handling(signal, (&state.action).into())?;

        let state = GrandState {
            current_setting: Setting::UserSpecified(state),
            parent_setting: None,
            internal_handler_enabled: false,
        };
        #[allow(clippy::drop_ref)]
        match entry {
            Entry::Vacant(vacant) => drop(vacant.insert(state)),
            Entry::Occupied(mut occupied) => drop(occupied.insert(state)),
        }

        Ok(())
    }

    fn clear_parent_settings(&mut self) {
        for state in self.traps.values_mut() {
            state.parent_setting = None;
        }
    }

    /// Returns an iterator over the signal actions configured in this trap set.
    ///
    /// The iterator yields tuples of the signal, the currently configured trap
    /// action, and the action set before
    /// [`enter_subshell`](Self::enter_subshell) was called.
    pub fn iter(&self) -> Iter<'_> {
        let inner = self.traps.iter();
        Iter { inner }
    }

    /// Resets existing `Trap::Command` settings to the default.
    ///
    /// POSIX requires that traps other than `Trap::Ignore` be reset when
    /// entering a subshell. This function achieves that effect.
    ///
    /// The trap set will remember the original trap states as the parent
    /// states. You can get them from the second return value of
    /// [`get_state`](Self::get_state) or the third item of tuples yielded by an
    /// [iterator](Self::iter).
    ///
    /// Note that trap actions other than `Trap::Command` remain as before.
    pub fn enter_subshell<S: SignalSystem>(&mut self, system: &mut S) {
        self.clear_parent_settings();

        for (cond, state) in &mut self.traps {
            let Setting::UserSpecified(trap) = &state.current_setting else { continue; };
            let Action::Command(_) = &trap.action else { continue; };

            state.parent_setting = Some(std::mem::replace(
                &mut state.current_setting,
                Setting::InitiallyDefaulted,
            ));

            let Condition::Signal(signal) = cond else { continue; };

            if !state.internal_handler_enabled {
                system
                    .set_signal_handling(*signal, crate::system::SignalHandling::Default)
                    .ok();
            }
        }
    }

    /// Sets the `pending` flag of the [`TrapState`] for the specified signal.
    ///
    /// This function does nothing if no trap action has been
    /// [set](Self::set_action) for the signal.
    pub fn catch_signal(&mut self, signal: Signal) {
        if let Some(state) = self.traps.get_mut(&Condition::Signal(signal)) {
            if let Setting::UserSpecified(trap) = &mut state.current_setting {
                trap.pending = true;
            }
        }
    }

    /// Returns a signal that has been [caught](Self::catch_signal).
    ///
    /// This function clears the `pending` flag of the [`TrapState`] for the
    /// specified signal.
    ///
    /// If there is more than one caught signal, it is unspecified which one of
    /// them is returned. If there is no caught signal, `None` is returned.
    pub fn take_caught_signal(&mut self) -> Option<(Signal, &TrapState)> {
        self.traps
            .iter_mut()
            .find_map(|(cond, state)| match (cond, &mut state.current_setting) {
                (Condition::Signal(signal), Setting::UserSpecified(trap)) if trap.pending => {
                    trap.pending = false;
                    Some((*signal, &*trap))
                }
                _ => None,
            })
    }

    /// Installs an internal handler for `SIGCHLD`.
    ///
    /// You should install the `SIGCHLD` handler to the system by using this
    /// function before waiting for `SIGCHLD` with [`System::wait`] and
    /// [`SharedSystem::wait_for_signal`].
    ///
    /// This function remembers that the handler has been installed, so a second
    /// call to the function will be a no-op.
    pub fn enable_sigchld_handler<S: SignalSystem>(&mut self, system: &mut S) -> Result<(), Errno> {
        let entry = self.traps.entry(Condition::Signal(Signal::SIGCHLD));
        if let Entry::Occupied(occupied) = &entry {
            if occupied.get().internal_handler_enabled {
                return Ok(());
            }
        }

        let previous_handler =
            system.set_signal_handling(Signal::SIGCHLD, SignalHandling::Catch)?;

        match entry {
            Entry::Occupied(mut occupied) => {
                occupied.get_mut().internal_handler_enabled = true;
            }
            Entry::Vacant(vacant) => {
                let current_setting = if previous_handler == SignalHandling::Ignore {
                    Setting::InitiallyIgnored
                } else {
                    Setting::InitiallyDefaulted
                };
                vacant.insert(GrandState {
                    current_setting,
                    parent_setting: None,
                    internal_handler_enabled: true,
                });
            }
        }

        Ok(())
    }

    /// Uninstalls all internal handlers.
    ///
    /// This function removes all internal handlers that have been previously
    /// installed by `self`. It leaves handlers for any existing user-defined
    /// traps.
    pub fn disable_internal_handlers<S: SignalSystem>(
        &mut self,
        system: &mut S,
    ) -> Result<(), Errno> {
        if let Some(state) = self.traps.get_mut(&Condition::Signal(Signal::SIGCHLD)) {
            if state.internal_handler_enabled {
                system.set_signal_handling(Signal::SIGCHLD, (&state.current_setting).into())?;
                state.internal_handler_enabled = false;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct DummySystem(HashMap<Signal, SignalHandling>);

    impl SignalSystem for DummySystem {
        fn set_signal_handling(
            &mut self,
            signal: Signal,
            handling: SignalHandling,
        ) -> Result<SignalHandling, Errno> {
            Ok(self
                .0
                .insert(signal, handling)
                .unwrap_or(SignalHandling::Default))
        }
    }

    #[test]
    fn condition_display() {
        assert_eq!(Condition::Exit.to_string(), "EXIT");
        assert_eq!(Condition::Signal(Signal::SIGINT).to_string(), "INT");
    }

    #[test]
    fn condition_from_str() {
        assert_eq!("EXIT".parse(), Ok(Condition::Exit));
        assert_eq!("TERM".parse(), Ok(Condition::Signal(Signal::SIGTERM)));
        assert_eq!("FOO".parse::<Condition>(), Err(ParseConditionError));
    }

    #[test]
    fn default_trap() {
        let trap_set = TrapSet::default();
        assert_eq!(trap_set.get_state(Signal::SIGCHLD), (None, None));
    }

    #[test]
    fn setting_trap_to_ignore() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("origin");

        let result = trap_set.set_action(
            &mut system,
            Signal::SIGCHLD,
            Action::Ignore,
            origin.clone(),
            false,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action: Action::Ignore,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Ignore
        );
    }

    #[test]
    fn setting_trap_to_command() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let action = Action::Command("echo".into());
        let origin = Location::dummy("origin");
        let result = trap_set.set_action(
            &mut system,
            Signal::SIGCHLD,
            action.clone(),
            origin.clone(),
            false,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Catch
        );
    }

    #[test]
    fn setting_trap_to_default() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("foo");
        trap_set
            .set_action(&mut system, Signal::SIGCHLD, Action::Ignore, origin, false)
            .unwrap();

        let origin = Location::dummy("bar");
        let result = trap_set.set_action(
            &mut system,
            Signal::SIGCHLD,
            Action::Default,
            origin.clone(),
            false,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action: Action::Default,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Default
        );
    }

    #[test]
    fn resetting_trap_from_ignore_no_override() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("foo");
        let result =
            trap_set.set_action(&mut system, Signal::SIGCHLD, Action::Ignore, origin, false);
        assert_eq!(result, Err(SetActionError::InitiallyIgnored));

        // Idempotence
        let origin = Location::dummy("bar");
        let result =
            trap_set.set_action(&mut system, Signal::SIGCHLD, Action::Ignore, origin, false);
        assert_eq!(result, Err(SetActionError::InitiallyIgnored));

        assert_eq!(trap_set.get_state(Signal::SIGCHLD), (None, None));
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Ignore
        );
    }

    #[test]
    fn resetting_trap_from_ignore_override() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("origin");
        let result = trap_set.set_action(
            &mut system,
            Signal::SIGCHLD,
            Action::Ignore,
            origin.clone(),
            true,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action: Action::Ignore,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Ignore
        );
    }

    #[test]
    fn setting_trap_for_two_signals() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin_1 = Location::dummy("foo");
        let result = trap_set.set_action(
            &mut system,
            Signal::SIGUSR1,
            Action::Ignore,
            origin_1.clone(),
            false,
        );
        assert_eq!(result, Ok(()));

        let command = Action::Command("echo".into());
        let origin_2 = Location::dummy("bar");
        let result = trap_set.set_action(
            &mut system,
            Signal::SIGUSR2,
            command.clone(),
            origin_2.clone(),
            false,
        );
        assert_eq!(result, Ok(()));

        assert_eq!(
            trap_set.get_state(Signal::SIGUSR1),
            (
                Some(&TrapState {
                    action: Action::Ignore,
                    origin: origin_1,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            trap_set.get_state(Signal::SIGUSR2),
            (
                Some(&TrapState {
                    action: command,
                    origin: origin_2,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGUSR1],
            crate::system::SignalHandling::Ignore
        );
        assert_eq!(
            system.0[&Signal::SIGUSR2],
            crate::system::SignalHandling::Catch
        );
    }

    #[test]
    fn setting_trap_for_sigkill() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("origin");
        let result =
            trap_set.set_action(&mut system, Signal::SIGKILL, Action::Ignore, origin, false);
        assert_eq!(result, Err(SetActionError::SIGKILL));
        assert_eq!(trap_set.get_state(Signal::SIGKILL), (None, None));
        assert_eq!(system.0.get(&Signal::SIGKILL), None);
    }

    #[test]
    fn setting_trap_for_sigstop() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("origin");
        let result =
            trap_set.set_action(&mut system, Signal::SIGSTOP, Action::Ignore, origin, false);
        assert_eq!(result, Err(SetActionError::SIGSTOP));
        assert_eq!(trap_set.get_state(Signal::SIGSTOP), (None, None));
        assert_eq!(system.0.get(&Signal::SIGSTOP), None);
    }

    #[test]
    fn basic_iteration() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin_1 = Location::dummy("foo");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGUSR1,
                Action::Ignore,
                origin_1.clone(),
                false,
            )
            .unwrap();
        let command = Action::Command("echo".into());
        let origin_2 = Location::dummy("bar");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGUSR2,
                command.clone(),
                origin_2.clone(),
                false,
            )
            .unwrap();

        let mut i = trap_set.iter();
        let first = i.next().unwrap();
        assert_eq!(first.0, &Signal::SIGUSR1);
        assert_eq!(first.1.unwrap().action, Action::Ignore);
        assert_eq!(first.1.unwrap().origin, origin_1);
        assert_eq!(first.2, None);
        let second = i.next().unwrap();
        assert_eq!(second.0, &Signal::SIGUSR2);
        assert_eq!(second.1.unwrap().action, command);
        assert_eq!(second.1.unwrap().origin, origin_2);
        assert_eq!(first.2, None);
        assert_eq!(i.next(), None);
    }

    #[test]
    fn iteration_after_entering_subshell() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin_1 = Location::dummy("foo");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGUSR1,
                Action::Ignore,
                origin_1.clone(),
                false,
            )
            .unwrap();
        let command = Action::Command("echo".into());
        let origin_2 = Location::dummy("bar");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGUSR2,
                command.clone(),
                origin_2.clone(),
                false,
            )
            .unwrap();
        trap_set.enter_subshell(&mut system);

        let mut i = trap_set.iter();
        let first = i.next().unwrap();
        assert_eq!(first.0, &Signal::SIGUSR1);
        assert_eq!(first.1.unwrap().action, Action::Ignore);
        assert_eq!(first.1.unwrap().origin, origin_1);
        assert_eq!(first.2, None);
        let second = i.next().unwrap();
        assert_eq!(second.0, &Signal::SIGUSR2);
        assert_eq!(second.1, None);
        assert_eq!(second.2.unwrap().action, command);
        assert_eq!(second.2.unwrap().origin, origin_2);
        assert_eq!(i.next(), None);
    }

    #[test]
    fn iteration_after_setting_trap_in_subshell() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin_1 = Location::dummy("foo");
        let command = Action::Command("echo".into());
        trap_set
            .set_action(&mut system, Signal::SIGUSR1, command, origin_1, false)
            .unwrap();
        trap_set.enter_subshell(&mut system);
        let origin_2 = Location::dummy("bar");
        let command = Action::Command("ls".into());
        trap_set
            .set_action(
                &mut system,
                Signal::SIGUSR2,
                command.clone(),
                origin_2.clone(),
                false,
            )
            .unwrap();

        let mut i = trap_set.iter();
        let first = i.next().unwrap();
        assert_eq!(first.0, &Signal::SIGUSR2);
        assert_eq!(first.1.unwrap().action, command);
        assert_eq!(first.1.unwrap().origin, origin_2);
        assert_eq!(first.2, None);
        assert_eq!(i.next(), None);
    }

    #[test]
    fn entering_subshell_resets_command_traps() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let action = Action::Command("".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGCHLD,
                action.clone(),
                origin.clone(),
                false,
            )
            .unwrap();

        trap_set.enter_subshell(&mut system);
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                None,
                Some(&TrapState {
                    action,
                    origin,
                    pending: false
                })
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Default
        );
    }

    #[test]
    fn entering_subshell_keeps_ignore_traps() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin = Location::dummy("origin");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGCHLD,
                Action::Ignore,
                origin.clone(),
                false,
            )
            .unwrap();

        trap_set.enter_subshell(&mut system);
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action: Action::Ignore,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Ignore
        );
    }

    #[test]
    fn entering_subshell_with_internal_handler() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let action = Action::Command("".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGCHLD,
                action.clone(),
                origin.clone(),
                false,
            )
            .unwrap();
        trap_set.enable_sigchld_handler(&mut system).unwrap();

        trap_set.enter_subshell(&mut system);
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                None,
                Some(&TrapState {
                    action,
                    origin,
                    pending: false
                })
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Catch
        );
    }

    #[test]
    fn setting_trap_after_entering_subshell_clears_parent_states() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin_1 = Location::dummy("foo");
        let command = Action::Command("echo 1".into());
        trap_set
            .set_action(&mut system, Signal::SIGUSR1, command, origin_1, false)
            .unwrap();
        let origin_2 = Location::dummy("bar");
        let command = Action::Command("echo 2".into());
        trap_set
            .set_action(&mut system, Signal::SIGUSR2, command, origin_2, false)
            .unwrap();
        trap_set.enter_subshell(&mut system);

        let command = Action::Command("echo 9".into());
        let origin_3 = Location::dummy("qux");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGUSR1,
                command.clone(),
                origin_3.clone(),
                false,
            )
            .unwrap();

        assert_eq!(
            trap_set.get_state(Signal::SIGUSR1),
            (
                Some(&TrapState {
                    action: command,
                    origin: origin_3,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(trap_set.get_state(Signal::SIGUSR2), (None, None));
        assert_eq!(
            system.0[&Signal::SIGUSR1],
            crate::system::SignalHandling::Catch
        );
        assert_eq!(
            system.0[&Signal::SIGUSR2],
            crate::system::SignalHandling::Default
        );
    }

    #[test]
    fn entering_nested_subshell_clears_parent_states() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let origin_1 = Location::dummy("foo");
        let command = Action::Command("echo 1".into());
        trap_set
            .set_action(&mut system, Signal::SIGUSR1, command, origin_1, false)
            .unwrap();
        let origin_2 = Location::dummy("bar");
        let command = Action::Command("echo 2".into());
        trap_set
            .set_action(&mut system, Signal::SIGUSR2, command, origin_2, false)
            .unwrap();
        trap_set.enter_subshell(&mut system);
        trap_set.enter_subshell(&mut system);

        assert_eq!(trap_set.get_state(Signal::SIGUSR1), (None, None));
        assert_eq!(trap_set.get_state(Signal::SIGUSR2), (None, None));
        assert_eq!(
            system.0[&Signal::SIGUSR1],
            crate::system::SignalHandling::Default
        );
        assert_eq!(
            system.0[&Signal::SIGUSR2],
            crate::system::SignalHandling::Default
        );
    }

    #[test]
    fn catching_signal() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        let command = Action::Command("echo INT".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(&mut system, Signal::SIGINT, command, origin, false)
            .unwrap();
        let command = Action::Command("echo TERM".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(&mut system, Signal::SIGTERM, command, origin, false)
            .unwrap();

        trap_set.catch_signal(Signal::SIGCHLD);
        trap_set.catch_signal(Signal::SIGINT);

        let trap_state = trap_set.get_state(Signal::SIGINT).0.unwrap();
        assert!(trap_state.pending, "{:?}", trap_state);
        let trap_state = trap_set.get_state(Signal::SIGTERM).0.unwrap();
        assert!(!trap_state.pending, "{:?}", trap_state);
    }

    #[test]
    fn taking_caught_signal() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        assert_eq!(trap_set.take_caught_signal(), None);

        let command = Action::Command("echo INT".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(&mut system, Signal::SIGINT, command, origin, false)
            .unwrap();
        let command = Action::Command("echo TERM".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(&mut system, Signal::SIGTERM, command, origin, false)
            .unwrap();
        let command = Action::Command("echo USR1".into());
        let origin = Location::dummy("origin");
        trap_set
            .set_action(&mut system, Signal::SIGUSR1, command, origin, false)
            .unwrap();
        assert_eq!(trap_set.take_caught_signal(), None);

        trap_set.catch_signal(Signal::SIGINT);
        trap_set.catch_signal(Signal::SIGUSR1);
        // The order in which take_caught_signal returns the two signals is
        // unspecified, so we accept both the orders.
        let result = trap_set.take_caught_signal().unwrap();
        match result.0 {
            Signal::SIGINT => {
                assert_eq!(result.1.action, Action::Command("echo INT".into()));
                assert!(!result.1.pending);

                let result = trap_set.take_caught_signal().unwrap();
                assert_eq!(result.0, Signal::SIGUSR1);
                assert_eq!(result.1.action, Action::Command("echo USR1".into()));
                assert!(!result.1.pending);
            }
            Signal::SIGUSR1 => {
                assert_eq!(result.1.action, Action::Command("echo USR1".into()));
                assert!(!result.1.pending);

                let result = trap_set.take_caught_signal().unwrap();
                assert_eq!(result.0, Signal::SIGINT);
                assert_eq!(result.1.action, Action::Command("echo INT".into()));
                assert!(!result.1.pending);
            }
            _ => panic!("wrong signal: {:?}", result),
        }

        assert_eq!(trap_set.take_caught_signal(), None);
    }

    #[test]
    fn enabling_sigchld_handler() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Catch);
    }

    #[test]
    fn disabling_internal_handler_for_initially_defaulted_sigchld() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        trap_set.disable_internal_handlers(&mut system).unwrap();
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Default);
    }

    #[test]
    fn disabling_internal_handler_for_initially_ignored_sigchld() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        trap_set.disable_internal_handlers(&mut system).unwrap();
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Ignore);
    }

    #[test]
    fn disabling_internal_handler_after_enabling_twice() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        trap_set.disable_internal_handlers(&mut system).unwrap();
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Ignore);
    }

    #[test]
    fn disabling_internal_handler_without_enabling() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        trap_set.disable_internal_handlers(&mut system).unwrap();
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Ignore);
    }

    #[test]
    fn reenabling_internal_handler() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        trap_set.disable_internal_handlers(&mut system).unwrap();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Catch);
    }

    #[test]
    fn setting_trap_to_ignore_after_enabling_internal_handler() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        let origin = Location::dummy("origin");
        let result =
            trap_set.set_action(&mut system, Signal::SIGCHLD, Action::Ignore, origin, false);
        assert_eq!(result, Ok(()));
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Catch);
    }

    #[test]
    fn resetting_trap_from_ignore_no_override_after_enabling_internal_handler() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        let origin = Location::dummy("origin");
        let result =
            trap_set.set_action(&mut system, Signal::SIGCHLD, Action::Ignore, origin, false);
        assert_eq!(result, Err(SetActionError::InitiallyIgnored));
        assert_eq!(system.0[&Signal::SIGCHLD], SignalHandling::Catch);
    }

    #[test]
    fn resetting_trap_from_ignore_override_after_enabling_internal_handler() {
        let mut system = DummySystem::default();
        system.0.insert(Signal::SIGCHLD, SignalHandling::Ignore);
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        let origin = Location::dummy("origin");
        let result = trap_set.set_action(
            &mut system,
            Signal::SIGCHLD,
            Action::Ignore,
            origin.clone(),
            true,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action: Action::Ignore,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Catch
        );
    }

    #[test]
    fn disabling_internal_handler_with_ignore_trap() {
        let mut system = DummySystem::default();
        let mut trap_set = TrapSet::default();
        trap_set.enable_sigchld_handler(&mut system).unwrap();
        let origin = Location::dummy("origin");
        trap_set
            .set_action(
                &mut system,
                Signal::SIGCHLD,
                Action::Ignore,
                origin.clone(),
                false,
            )
            .unwrap();
        trap_set.disable_internal_handlers(&mut system).unwrap();

        assert_eq!(
            trap_set.get_state(Signal::SIGCHLD),
            (
                Some(&TrapState {
                    action: Action::Ignore,
                    origin,
                    pending: false
                }),
                None
            )
        );
        assert_eq!(
            system.0[&Signal::SIGCHLD],
            crate::system::SignalHandling::Ignore
        );
    }
}
