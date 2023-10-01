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

//! Command line argument parser for the cd built-in

use super::Command;
use super::Mode;
use crate::common::syntax::parse_arguments;
use crate::common::syntax::OptionOccurrence;
use crate::common::syntax::OptionSpec;
use std::borrow::Cow;
use std::collections::VecDeque;
use thiserror::Error;
use yash_env::semantics::Field;
use yash_env::Env;
use yash_syntax::source::pretty::Annotation;
use yash_syntax::source::pretty::AnnotationType;
use yash_syntax::source::pretty::MessageBase;

/// Error in parsing command line arguments
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// An error occurred in the common parser.
    #[error(transparent)]
    CommonError(#[from] crate::common::syntax::ParseError<'static>),

    /// The operand is an empty string.
    // TODO: EmptyOperand(Field),

    /// More than one operand is given.
    #[error("unexpected operand")]
    UnexpectedOperands(Vec<Field>),
}

impl MessageBase for Error {
    fn message_title(&self) -> Cow<str> {
        self.to_string().into()
    }

    fn main_annotation(&self) -> Annotation<'_> {
        use Error::*;
        match self {
            CommonError(e) => e.main_annotation(),
            UnexpectedOperands(operands) => Annotation::new(
                AnnotationType::Error,
                format!("{}: unexpected operand", operands[0].value).into(),
                &operands[0].origin,
            ),
        }
    }
}

/// Result of parsing command line arguments
pub type Result = std::result::Result<Command, Error>;

const OPTION_SPECS: &[OptionSpec] = &[
    OptionSpec::new().short('L').long("logical"),
    OptionSpec::new().short('P').long("physical"),
];

fn mode_for_option(option: &OptionOccurrence) -> Mode {
    match option.spec.get_short() {
        Some('L') => Mode::Logical,
        Some('P') => Mode::Physical,
        _ => unreachable!(),
    }
}

/// Parses command line arguments for the cd built-in.
pub fn parse(env: &Env, args: Vec<Field>) -> Result {
    let parser_mode = crate::common::syntax::Mode::with_env(env);
    let (options, operands) = parse_arguments(OPTION_SPECS, parser_mode, args)?;

    let mode = options.last().map(mode_for_option).unwrap_or_default();
    let mut operands = VecDeque::from(operands);
    let operand = operands.pop_front();
    if operands.is_empty() {
        Ok(Command { mode, operand })
    } else {
        Err(Error::UnexpectedOperands(operands.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments() {
        let env = Env::new_virtual();
        let result = parse(&env, vec![]);
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Logical,
                operand: None,
            })
        );
    }

    #[test]
    fn logical_option() {
        let env = Env::new_virtual();
        let result = parse(&env, Field::dummies(["-L"]));
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Logical,
                operand: None,
            })
        );
    }

    #[test]
    fn physical_option() {
        let env = Env::new_virtual();
        let result = parse(&env, Field::dummies(["-P"]));
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Physical,
                operand: None,
            })
        );
    }

    #[test]
    fn last_option_wins() {
        let env = Env::new_virtual();

        let result = parse(&env, Field::dummies(["-L", "-P"]));
        assert_eq!(result.unwrap().mode, Mode::Physical);

        let result = parse(&env, Field::dummies(["-P", "-L"]));
        assert_eq!(result.unwrap().mode, Mode::Logical);

        let result = parse(&env, Field::dummies(["-L", "-P", "-L"]));
        assert_eq!(result.unwrap().mode, Mode::Logical);

        let result = parse(&env, Field::dummies(["-PLP"]));
        assert_eq!(result.unwrap().mode, Mode::Physical);
    }

    #[test]
    fn with_operand() {
        let env = Env::new_virtual();
        let operand = Field::dummy("foo/bar");
        let result = parse(&env, vec![operand.clone()]);
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::default(),
                operand: Some(operand),
            })
        );
    }

    #[test]
    fn option_and_operand() {
        let env = Env::new_virtual();
        let operand = Field::dummy("foo/bar");
        let args = vec![Field::dummy("-L"), Field::dummy("--"), operand.clone()];
        let result = parse(&env, args);
        assert_eq!(
            result,
            Ok(Command {
                mode: Mode::Logical,
                operand: Some(operand),
            })
        );
    }

    #[test]
    fn unexpected_operand() {
        let env = Env::new_virtual();
        let operand1 = Field::dummy("foo");
        let operand2 = Field::dummy("bar");
        let result = parse(&env, vec![operand1, operand2.clone()]);
        assert_eq!(result, Err(Error::UnexpectedOperands(vec![operand2])));
    }

    #[test]
    fn unexpected_operands_after_options() {
        let env = Env::new_virtual();
        let args = Field::dummies(["-LP", "-L", "--", "one", "two", "three"]);
        let extra_operands = args[4..].to_vec();
        let result = parse(&env, args);
        assert_eq!(result, Err(Error::UnexpectedOperands(extra_operands)));
    }
}
