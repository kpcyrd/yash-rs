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

//! Implementations for Command.

mod and_or_list;
mod compound_command;
mod function_definition;
mod item;
mod pipeline;
mod simple_command;

use super::Command;
use crate::trap::run_traps_for_caught_signals;
use async_trait::async_trait;
use std::ops::ControlFlow::{Break, Continue};
use yash_env::semantics::Result;
use yash_env::Env;
use yash_syntax::syntax;

/// Executes the command.
///
/// After executing the command body, the `execute` function [runs
/// traps](run_traps_for_caught_signals) if any caught signals are pending, and
/// [updates subshell statuses](Env::update_all_subshell_statuses).
#[async_trait(?Send)]
impl Command for syntax::Command {
    async fn execute(&self, env: &mut Env) -> Result {
        use syntax::Command::*;
        let main_result = match self {
            Simple(command) => command.execute(env).await,
            Compound(command) => command.execute(env).await,
            Function(definition) => definition.execute(env).await,
        };

        let trap_result = run_traps_for_caught_signals(env).await;
        env.update_all_subshell_statuses();

        match (main_result, trap_result) {
            (_, Continue(())) => main_result,
            (Continue(()), _) => trap_result,
            (Break(main_divert), Break(trap_divert)) => Break(main_divert.max(trap_divert)),
        }
    }
}

/// Executes the list.
///
/// The list is executed by executing each item in sequence. If any item results
/// in a [`Divert`](yash_env::semantics::Divert), the remaining items are not
/// executed.
#[async_trait(?Send)]
impl Command for syntax::List {
    async fn execute(&self, env: &mut Env) -> Result {
        for item in &self.0 {
            item.execute(env).await?
        }
        Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::echo_builtin;
    use crate::tests::return_builtin;
    use futures_executor::block_on;
    use yash_env::semantics::Divert;
    use yash_env::semantics::ExitStatus;
    use yash_env::trap::Signal;
    use yash_env::trap::Trap;
    use yash_env::VirtualSystem;
    use yash_syntax::source::Location;

    #[test]
    fn command_handles_traps() {
        let system = VirtualSystem::new();
        let mut env = Env::with_system(Box::new(system.clone()));
        env.builtins.insert("echo", echo_builtin());
        env.traps
            .set_trap(
                &mut env.system,
                Signal::SIGUSR1,
                Trap::Command("echo USR1".into()),
                Location::dummy(""),
                false,
            )
            .unwrap();
        system
            .state
            .borrow_mut()
            .processes
            .get_mut(&system.process_id)
            .unwrap()
            .raise_signal(Signal::SIGUSR1);
        let command: syntax::Command = "echo main".parse().unwrap();
        let result = block_on(command.execute(&mut env));
        assert_eq!(result, Continue(()));
        assert_eq!(env.exit_status, ExitStatus::SUCCESS);
        assert_eq!(
            system
                .state
                .borrow()
                .file_system
                .get("/dev/stdout")
                .unwrap()
                .borrow()
                .content,
            b"main\nUSR1\n"
        );
    }

    #[test]
    fn list_execute_no_divert() {
        let mut env = Env::new_virtual();
        env.builtins.insert("return", return_builtin());
        let list: syntax::List = "return -n 1; return -n 2; return -n 4".parse().unwrap();
        let result = block_on(list.execute(&mut env));
        assert_eq!(result, Continue(()));
        assert_eq!(env.exit_status, ExitStatus(4));
    }

    #[test]
    fn list_execute_divert() {
        let mut env = Env::new_virtual();
        env.builtins.insert("return", return_builtin());
        let list: syntax::List = "return -n 1; return 2; return -n 4".parse().unwrap();
        let result = block_on(list.execute(&mut env));
        assert_eq!(result, Break(Divert::Return));
        assert_eq!(env.exit_status, ExitStatus(2));
    }
}
