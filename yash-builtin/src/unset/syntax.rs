// This file is part of yash, an extended POSIX shell.
// Copyright (C) 2023 WATANABE Yuki
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

//! Parses the unset built-in command arguments.

use crate::common::syntax::parse_arguments;
use crate::common::syntax::OptionOccurrence;
use crate::common::syntax::OptionSpec;
use std::borrow::Cow;
use thiserror::Error;
use yash_env::semantics::Field;
use yash_env::Env;
use yash_syntax::source::pretty::Annotation;
use yash_syntax::source::pretty::MessageBase;

use super::Command;
use super::Mode;

/// Error in parsing command line arguments
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// An error occurred in the common parser.
    #[error(transparent)]
    CommonError(#[from] crate::common::syntax::Error<'static>),
    // TODO MissingOperand
}

impl MessageBase for Error {
    fn message_title(&self) -> Cow<str> {
        self.to_string().into()
    }

    fn main_annotation(&self) -> Annotation<'_> {
        match self {
            Error::CommonError(inner) => inner.main_annotation(),
        }
    }
}

/// Result of parsing command line arguments
pub type Result = std::result::Result<Command, Error>;

const OPTION_SPECS: &[OptionSpec] = &[
    OptionSpec::new().short('f').long("functions"),
    OptionSpec::new().short('v').long("variables"),
];

fn mode_for_option(option: &OptionOccurrence) -> Mode {
    match option.spec.get_short() {
        Some('f') => Mode::Functions,
        Some('v') => Mode::Variables,
        _ => unreachable!("{option:?}"),
    }
}

/// Parses command line arguments for the unset built-in.
pub fn parse(env: &Env, args: Vec<Field>) -> Result {
    let parser_mode = crate::common::syntax::Mode::with_env(env);
    let (options, operands) = parse_arguments(OPTION_SPECS, parser_mode, args)?;

    Ok(Command {
        mode: options.last().map(mode_for_option).unwrap_or_default(),
        names: operands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_non_posix() {
        let env = Env::new_virtual();
        let result = parse(&env, vec![]);
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Variables,
                names: vec![],
            })
        );
    }

    // TODO no_arguments_posix: In the POSIXly-correct mode, the built-in
    // requires at least one operand.

    #[test]
    fn v_option() {
        let env = Env::new_virtual();
        let result = parse(&env, Field::dummies(["-v"]));
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Variables,
                names: vec![],
            })
        );

        // The same option can be specified multiple times.
        let result = parse(&env, Field::dummies(["-vv", "--variables"]));
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Variables,
                names: vec![],
            })
        );
    }

    #[test]
    fn f_option() {
        let env = Env::new_virtual();
        let result = parse(&env, Field::dummies(["-f"]));
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Functions,
                names: vec![],
            })
        );

        // The same option can be specified multiple times.
        let result = parse(&env, Field::dummies(["-ff", "--functions"]));
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Functions,
                names: vec![],
            })
        );
    }

    #[test]
    fn v_and_f_option() {
        // Specifying both -v and -f is an error.
        // TODO Common argument parser should detect this.
    }

    #[test]
    fn operands() {
        let env = Env::new_virtual();
        let args = Field::dummies(["foo", "bar"]);
        let result = parse(&env, args.clone());
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Variables,
                names: args,
            })
        );
    }
}
