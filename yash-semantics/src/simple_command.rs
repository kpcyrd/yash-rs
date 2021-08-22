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

//! Implementation of simple command semantics.

use super::Command;
use crate::command_search::search;
use crate::command_search::Target::{Builtin, External, Function};
use crate::expansion::expand_words;
use async_trait::async_trait;
use nix::errno::Errno;
use std::ffi::CString;
use yash_env::exec::ExitStatus;
use yash_env::exec::Result;
use yash_env::expansion::Field;
use yash_env::Env;
use yash_env::System;
use yash_syntax::syntax;

/// Converts fields to C strings.
fn to_c_strings(s: Vec<Field>) -> Vec<CString> {
    // TODO return something rather than dropping null-containing strings
    s.into_iter()
        .filter_map(|f| CString::new(f.value).ok())
        .collect()
}

#[async_trait(?Send)]
impl Command for syntax::SimpleCommand {
    /// Executes the simple command.
    ///
    /// TODO Elaborate
    ///
    /// POSIX does not define the exit status when the `execve` system call
    /// fails for a reason other than `ENOEXEC`. In this implementation, the
    /// exit status is 127 for `ENOENT` and `ENOTDIR` and 126 for others.
    async fn execute(&self, env: &mut Env) -> Result {
        let fields = match expand_words(env, &self.words).await {
            Ok(fields) => fields,
            Err(error) => {
                env.print_error(&format_args!("expansion failure: {:?}", error))
                    .await;
                // TODO Handle errors that may happen in expansion
                return Ok(());
            }
        };

        // TODO open redirections
        // TODO expand and perform assignments

        if let Some(name) = fields.get(0) {
            match search(env, &name.value) {
                Some(Builtin(builtin)) => {
                    let (exit_status, abort) = (builtin.execute)(env, fields).await;
                    env.exit_status = exit_status;
                    if let Some(abort) = abort {
                        return Err(abort);
                    }
                }
                Some(Function(function)) => {
                    println!("Function: {:?}", function);
                    // TODO Call the function
                }
                Some(External { path }) => {
                    let args = to_c_strings(fields);
                    let envs = env.variables.env_c_strings();
                    let result = env
                        .run_in_subshell(move |env| {
                            Box::pin(async move {
                                // TODO Remove signal handlers not set by current traps

                                let result = env.system.execve(path.as_c_str(), &args, &envs);
                                // TODO Prefer into_err to unwrap_err
                                let errno = result.unwrap_err();
                                // TODO Reopen as shell script on ENOEXEC
                                match errno {
                                    Errno::ENOENT | Errno::ENOTDIR => {
                                        env.exit_status = ExitStatus::NOT_FOUND;
                                    }
                                    _ => {
                                        env.exit_status = ExitStatus::NOEXEC;
                                    }
                                }
                                env.print_system_error(
                                    errno,
                                    &format_args!("cannot execute external command {:?}", path),
                                )
                                .await
                            })
                        })
                        .await;

                    match result {
                        Ok(exit_status) => {
                            env.exit_status = exit_status;
                        }
                        Err(errno) => {
                            env.print_system_error(
                                errno,
                                &format_args!("cannot execute external command"),
                            )
                            .await;
                            env.exit_status = ExitStatus::NOEXEC;
                        }
                    }
                }
                None => {
                    env.print_error(&format_args!("{}: command not found", name.value))
                        .await;
                    env.exit_status = ExitStatus::NOT_FOUND;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::return_builtin;
    use crate::tests::LocalExecutor;
    use futures_executor::block_on;
    use futures_executor::LocalPool;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use yash_env::exec::Divert;
    use yash_env::variable::Value;
    use yash_env::variable::Variable;
    use yash_env::virtual_system::INode;
    use yash_env::VirtualSystem;

    #[test]
    fn simple_command_returns_exit_status_from_builtin_without_divert() {
        let mut env = Env::new_virtual();
        env.builtins.insert("return", return_builtin());
        let command: syntax::SimpleCommand = "return -n 93".parse().unwrap();
        let result = block_on(command.execute(&mut env));
        assert_eq!(result, Ok(()));
        assert_eq!(env.exit_status, ExitStatus(93));
    }

    #[test]
    fn simple_command_returns_exit_status_from_builtin_with_divert() {
        let mut env = Env::new_virtual();
        env.builtins.insert("return", return_builtin());
        let command: syntax::SimpleCommand = "return 37".parse().unwrap();
        let result = block_on(command.execute(&mut env));
        assert_eq!(result, Err(Divert::Return));
        assert_eq!(env.exit_status, ExitStatus(37));
    }

    #[test]
    fn simple_command_calls_execve_with_correct_arguments() {
        let system = VirtualSystem::new();
        let state = Rc::clone(&system.state);

        let path = PathBuf::from("/some/file");
        let mut content = INode::default();
        let mut executor = LocalPool::new();
        content.permissions.0 |= 0o100;
        content.is_native_executable = true;
        let content = Rc::new(RefCell::new(content));
        system.state.borrow_mut().file_system.save(path, content);
        system.state.borrow_mut().executor = Some(Rc::new(LocalExecutor(executor.spawner())));

        let mut env = Env::with_system(Box::new(system));
        env.variables.assign(
            "env".to_string(),
            Variable {
                value: Value::Scalar("scalar".to_string()),
                last_assigned_location: None,
                is_exported: true,
                read_only_location: None,
            },
        );
        env.variables.assign(
            "local".to_string(),
            Variable {
                value: Value::Scalar("ignored".to_string()),
                last_assigned_location: None,
                is_exported: false,
                read_only_location: None,
            },
        );
        let command: syntax::SimpleCommand = "/some/file foo bar".parse().unwrap();
        let result = executor.run_until(command.execute(&mut env));
        assert_eq!(result, Ok(()));

        let state = state.borrow();
        let process = state.processes.values().last().unwrap();
        let arguments = process.last_exec().as_ref().unwrap();
        assert_eq!(arguments.0, CString::new("/some/file").unwrap());
        assert_eq!(
            arguments.1,
            [
                CString::new("/some/file").unwrap(),
                CString::new("foo").unwrap(),
                CString::new("bar").unwrap()
            ]
        );
        assert_eq!(arguments.2, [CString::new("env=scalar").unwrap()]);
    }

    #[test]
    fn simple_command_returns_exit_status_from_external_utility() {
        let system = VirtualSystem::new();
        let path = PathBuf::from("/some/file");
        let mut content = INode::default();
        let mut executor = LocalPool::new();
        content.permissions.0 |= 0o100;
        content.is_native_executable = true;
        let content = Rc::new(RefCell::new(content));
        system.state.borrow_mut().file_system.save(path, content);
        system.state.borrow_mut().executor = Some(Rc::new(LocalExecutor(executor.spawner())));

        let mut env = Env::with_system(Box::new(system));
        let command: syntax::SimpleCommand = "/some/file foo bar".parse().unwrap();
        let result = executor.run_until(command.execute(&mut env));
        assert_eq!(result, Ok(()));
        // In VirtualSystem, execve fails with ENOSYS.
        assert_eq!(env.exit_status, ExitStatus::NOEXEC);
    }

    #[test]
    fn simple_command_returns_127_for_non_existing_file() {
        let system = VirtualSystem::new();
        let mut executor = LocalPool::new();
        system.state.borrow_mut().executor = Some(Rc::new(LocalExecutor(executor.spawner())));

        let mut env = Env::with_system(Box::new(system));
        let command: syntax::SimpleCommand = "/some/file".parse().unwrap();
        let result = executor.run_until(command.execute(&mut env));
        assert_eq!(result, Ok(()));
        assert_eq!(env.exit_status, ExitStatus::NOT_FOUND);
    }

    #[test]
    fn simple_command_returns_126_on_exec_failure() {
        let system = VirtualSystem::new();
        let path = PathBuf::from("/some/file");
        let mut content = INode::default();
        let mut executor = LocalPool::new();
        content.permissions.0 |= 0o100;
        let content = Rc::new(RefCell::new(content));
        system.state.borrow_mut().file_system.save(path, content);
        system.state.borrow_mut().executor = Some(Rc::new(LocalExecutor(executor.spawner())));

        let mut env = Env::with_system(Box::new(system));
        let command: syntax::SimpleCommand = "/some/file".parse().unwrap();
        let result = executor.run_until(command.execute(&mut env));
        assert_eq!(result, Ok(()));
        assert_eq!(env.exit_status, ExitStatus::NOEXEC);
    }

    #[test]
    fn simple_command_returns_126_on_fork_failure() {
        let mut env = Env::new_virtual();
        let command: syntax::SimpleCommand = "/some/file".parse().unwrap();
        let result = block_on(command.execute(&mut env));
        assert_eq!(result, Ok(()));
        assert_eq!(env.exit_status, ExitStatus::NOEXEC);
    }

    #[test]
    fn exit_status_is_127_on_command_not_found() {
        let mut env = Env::new_virtual();
        let command: syntax::SimpleCommand = "no_such_command".parse().unwrap();
        let result = block_on(command.execute(&mut env));
        assert_eq!(result, Ok(()));
        assert_eq!(env.exit_status, ExitStatus::NOT_FOUND);
    }
}
