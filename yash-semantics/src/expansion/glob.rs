// This file is part of yash, an extended POSIX shell.
// Copyright (C) 2022 WATANABE Yuki
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

//! Pathname expansion
//!
//! Pathname expansion (a.k.a. globbing) scans directories and produces
//! pathnames matching the input field.
//!
//! # Pattern syntax
//!
//! An input field is split by `/`, and each component is parsed as a pattern
//! that may contain the following non-literal elements:
//!
//! - `?`
//! - `*`
//! - Bracket expression (a set of characters enclosed in brackets)
//!
//! Refer to the [`yash-fnmatch`](yash_fnmatch) crate for pattern syntax and
//! semantics details.
//!
//! # Directory scanning
//!
//! The expansion scans directories corresponding to components containing any
//! non-literal elements above. The scan requires read permission for the
//! directory. For components that have only literal characters, no scan is
//! performed. Search permissions for all ancestor directories are needed to
//! check if the file exists referred to by the resulting pathname.
//!
//! # Results
//!
//! Pathname expansion returns pathnames that have matched the input pattern,
//! sorted alphabetically. Any errors are silently ignored. If directory
//! scanning produces no pathnames, the input pattern is returned intact. (TODO:
//! the null-glob option)
//!
//! If the input field contains no non-literal elements subject to pattern
//! matching at all, the result is the input intact.

use super::attr::AttrField;
use std::marker::PhantomData;
use yash_env::semantics::Field;
use yash_env::Env;

/// Iterator that provides results of parameter expansion
///
/// This iterator is created with the [`glob`] function.
pub struct Glob<'a> {
    /// Dummy to allow retaining a mutable reference to `Env` in the future
    ///
    /// The current [`glob`] implementation pre-computes all results before
    /// returning a `Glob`. The future implementation may optimize by using a
    /// [generator], which will need a real reference to `Env`.
    ///
    /// [generator]: https://github.com/rust-lang/rust/issues/43122
    env: PhantomData<&'a mut Env>,
}

impl Iterator for Glob<'_> {
    type Item = Field;
    fn next(&mut self) -> Option<Field> {
        // TODO
        None
    }
}

/// Performs parameter expansion.
///
/// This function returns an iterator that yields fields resulting from the
/// expansion.
pub fn glob(_env: &mut Env, _field: AttrField) -> Glob {
    Glob { env: PhantomData }
}
