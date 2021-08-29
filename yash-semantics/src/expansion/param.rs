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

//! Parameter expansion semantics.

use super::Env;
use super::Expand;
use super::Expander;
use super::Expansion;
use super::Origin;
use super::Result;
use async_trait::async_trait;
use yash_env::variable::Value;
use yash_syntax::source::Location;

/// Reference to a `RawParam` or `BracedParam`.
pub struct ParamRef<'a> {
    name: &'a str,
    location: &'a Location,
}

impl<'a> ParamRef<'a> {
    pub fn from_name_and_location(name: &'a str, location: &'a Location) -> Self {
        ParamRef { name, location }
    }
}

#[async_trait(?Send)]
impl Expand for ParamRef<'_> {
    async fn expand<E: Env>(&self, _e: &mut Expander<'_, E>) -> Result {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::AttrChar;
    use super::*;
    use futures_executor::block_on;
    use yash_env::variable::Value;
    use yash_env::variable::Variable;

    #[derive(Debug)]
    struct Singleton {
        name: String,
        value: Variable,
    }

    impl Env for Singleton {
        fn get_variable(&self, name: &str) -> Option<&Variable> {
            if name == self.name {
                Some(&self.value)
            } else {
                None
            }
        }
    }

    #[test]
    fn name_only_param_unset() {
        let name = "foo".to_string();
        let value = Variable {
            value: Value::Scalar("!".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: None,
        };
        let mut env = Singleton { name, value };
        let mut field = Vec::<AttrChar>::default();
        let mut e = Expander::new(&mut env, &mut field);
        let location = Location::dummy("");
        let p = ParamRef::from_name_and_location("bar", &location);
        block_on(p.expand(&mut e)).unwrap();
        assert_eq!(field, []);
    }
}
