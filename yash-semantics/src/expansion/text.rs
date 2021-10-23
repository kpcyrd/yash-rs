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

//! Initial expansion of text.

use super::command_subst::expand_command_substitution;
use super::param::ParamRef;
use super::AttrChar;
use super::Env;
use super::Expand;
use super::Expansion;
use super::Origin;
use super::Output;
use super::Result;
use async_trait::async_trait;
use yash_syntax::source::Location;
use yash_syntax::syntax::Text;
use yash_syntax::syntax::TextUnit;
use yash_syntax::syntax::Unquote;

#[async_trait(?Send)]
impl Expand for TextUnit {
    /// Expands the text unit.
    ///
    /// TODO Elaborate
    async fn expand<E: Env>(&self, env: &mut E, output: &mut Output<'_>) -> Result {
        /// Common part for command substitutions.
        async fn command_subst<E: Env>(
            env: &mut E,
            content: &str,
            location: &Location,
            output: &mut Output<'_>,
        ) -> Result {
            // TODO return exit_status
            let (result, _exit_status) =
                expand_command_substitution(env, content, location).await?;
            output.push_str(&result, Origin::SoftExpansion, false, false);
            Ok(())
        }

        use TextUnit::*;
        match self {
            Literal(c) => {
                output.push_char(AttrChar {
                    value: *c,
                    origin: Origin::Literal,
                    is_quoted: false,
                    is_quoting: false,
                });
                Ok(())
            }
            Backslashed(c) => {
                output.push_char(AttrChar {
                    value: '\\',
                    origin: Origin::Literal,
                    is_quoted: false,
                    is_quoting: true,
                });
                output.push_char(AttrChar {
                    value: *c,
                    origin: Origin::Literal,
                    is_quoted: true,
                    is_quoting: false,
                });
                Ok(())
            }
            RawParam { name, location } => {
                let param = ParamRef::from_name_and_location(name, location);
                param.expand(env, output).await
            }
            BracedParam(param) => ParamRef::from(param).expand(env, output).await,
            CommandSubst { content, location } => {
                command_subst(env, content, location, output).await
            }
            Backquote { content, location } => {
                let content = content.unquote().0;
                command_subst(env, &content, location, output).await
            }
            // TODO Expand Arith correctly
            _ => {
                output.push_str(&self.to_string(), Origin::Literal, false, false);
                Ok(())
            }
        }
    }
}

#[async_trait(?Send)]
impl Expand for Text {
    /// Expands the text.
    async fn expand<E: Env>(&self, env: &mut E, output: &mut Output<'_>) -> Result {
        self.0.expand(env, output).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::AttrChar;
    use super::*;
    use crate::expansion::tests::NullEnv;
    use crate::tests::echo_builtin;
    use crate::tests::in_virtual_system;
    use futures_executor::block_on;
    use yash_syntax::source::Location;
    use yash_syntax::syntax::TextUnit;

    #[test]
    fn literal_expand_unquoted() {
        let mut field = Vec::<AttrChar>::default();
        let mut env = NullEnv;
        let mut output = Output::new(&mut field);
        let l = TextUnit::Literal('&');
        block_on(l.expand(&mut env, &mut output)).unwrap();
        assert_eq!(
            field,
            [AttrChar {
                value: '&',
                origin: Origin::Literal,
                is_quoted: false,
                is_quoting: false
            }]
        );
    }

    #[test]
    fn backslashed_expand_unquoted() {
        let mut field = Vec::<AttrChar>::default();
        let mut env = NullEnv;
        let mut output = Output::new(&mut field);
        let b = TextUnit::Backslashed('$');
        block_on(b.expand(&mut env, &mut output)).unwrap();
        assert_eq!(
            field,
            [
                AttrChar {
                    value: '\\',
                    origin: Origin::Literal,
                    is_quoted: false,
                    is_quoting: true,
                },
                AttrChar {
                    value: '$',
                    origin: Origin::Literal,
                    is_quoted: true,
                    is_quoting: false,
                }
            ]
        );
    }

    #[test]
    fn command_subst_expand_unquoted() {
        in_virtual_system(|mut env, _pid, _state| async move {
            let mut field = Vec::<AttrChar>::default();
            let mut output = Output::new(&mut field);
            let subst = TextUnit::CommandSubst {
                content: "echo .".to_string(),
                location: Location::dummy(""),
            };
            env.builtins.insert("echo", echo_builtin());
            subst.expand(&mut env, &mut output).await.unwrap();
            assert_eq!(
                field,
                [AttrChar {
                    value: '.',
                    origin: Origin::SoftExpansion,
                    is_quoted: false,
                    is_quoting: false
                }]
            );
        })
    }

    #[test]
    fn backquote_expand_unquoted() {
        in_virtual_system(|mut env, _pid, _state| async move {
            use yash_syntax::syntax::BackquoteUnit::*;
            let mut field = Vec::<AttrChar>::default();
            let mut output = Output::new(&mut field);
            let subst = TextUnit::Backquote {
                content: vec![
                    Literal('e'),
                    Literal('c'),
                    Literal('h'),
                    Literal('o'),
                    Literal(' '),
                    Backslashed('\\'),
                    Backslashed('\\'),
                ],
                location: Location::dummy(""),
            };
            env.builtins.insert("echo", echo_builtin());
            subst.expand(&mut env, &mut output).await.unwrap();
            assert_eq!(
                field,
                [AttrChar {
                    value: '\\',
                    origin: Origin::SoftExpansion,
                    is_quoted: false,
                    is_quoting: false
                }]
            );
        })
    }

    #[test]
    fn text_expand() {
        let mut field = Vec::<AttrChar>::default();
        let mut env = NullEnv;
        let mut output = Output::new(&mut field);
        let text: Text = "<->".parse().unwrap();
        block_on(text.expand(&mut env, &mut output)).unwrap();
        assert_eq!(
            field,
            [
                AttrChar {
                    value: '<',
                    origin: Origin::Literal,
                    is_quoted: false,
                    is_quoting: false
                },
                AttrChar {
                    value: '-',
                    origin: Origin::Literal,
                    is_quoted: false,
                    is_quoting: false
                },
                AttrChar {
                    value: '>',
                    origin: Origin::Literal,
                    is_quoted: false,
                    is_quoting: false
                }
            ]
        );
    }
}
