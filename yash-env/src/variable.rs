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

//! Type definitions for shell variables.
//!
//! A [`VariableSet`] is a stack of contexts, and a _context_ is a map of
//! name-variable pairs. The `VariableSet` has a _base context_ with the same
//! lifetime as the `VariableSet` itself. Additional contexts can be added
//! (pushed) and removed (popped) on a last-in-first-out basis.
//!
//! You can define any number of [`Variable`]s in a context.
//! A new context is empty when pushed to the variable set.
//! You can pop a context regardless of whether it is empty or not;
//! all the variables in the popped context are removed as well.
//!
//! Variables in a context hide those with the same name in lower contexts. You
//! cannot access such hidden variables until removing the hiding variable from
//! the upper context.
//!
//! Each regular context has a special array variable called positional
//! parameters. Because it does not have a name as a variable, you need to use
//! dedicated methods for accessing it.
//! See [`VariableSet::positional_params`] and its [mut
//! variant](VariableSet::positional_params_mut).
//!
//! This module provides guards to ensure contexts are pushed and popped
//! correctly. The push function returns a guard that will pop the context when
//! dropped. Implementing `Deref` and `DerefMut`, the guard allows access to the
//! borrowed `VariableSet` or `Env`. [`VariableSet::push_context`] returns a
//! [`ContextGuard`] that allows re-borrowing the `VariableSet`.
//! [`Env::push_context`] returns a [`EnvContextGuard`] that implements
//! `DerefMut<Target = Env>`.

use crate::Env;
use either::{Left, Right};
use itertools::Itertools;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt::Write;
use std::hash::Hash;
use std::ops::Deref;
use std::ops::DerefMut;
use yash_syntax::source::Location;

/// Value of a variable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    /// Single string.
    Scalar(String),
    /// Array of strings.
    Array(Vec<String>),
}

pub use Value::*;

impl Value {
    /// Splits the value by colons.
    ///
    /// If this value is `Scalar`, the value is separated at each occurrence of
    /// colon (`:`). For `Array`, each array item is returned without further
    /// splitting the value.
    ///
    /// ```
    /// # use yash_env::variable::Value::Scalar;
    /// let scalar = Scalar("/usr/local/bin:/usr/bin:/bin".to_string());
    /// let values: Vec<&str> = scalar.split().collect();
    /// assert_eq!(values, ["/usr/local/bin", "/usr/bin", "/bin"]);
    /// ```
    ///
    /// ```
    /// # use yash_env::variable::Value::Array;
    /// let array = Array(vec!["foo".to_string(), "bar".to_string()]);
    /// let values: Vec<&str> = array.split().collect();
    /// assert_eq!(values, ["foo", "bar"]);
    /// ```
    pub fn split(&self) -> impl Iterator<Item = &str> {
        match self {
            Scalar(value) => Left(value.split(':')),
            Array(values) => Right(values.iter().map(String::as_str)),
        }
    }
}

/// Definition of a variable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variable {
    /// Value of the variable.
    pub value: Value,

    /// Optional location where this variable was assigned.
    ///
    /// If the current variable value originates from an assignment performed in
    /// the shell session, `last_assigned_location` is the location of the
    /// assignment.  Otherwise, `last_assigned_location` is `None`.
    pub last_assigned_location: Option<Location>,

    /// Whether this variable is exported or not.
    ///
    /// An exported variable is also referred to as an _environment variable_.
    pub is_exported: bool,

    /// Optional location where this variable was made read-only.
    ///
    /// If this variable is not read-only, `read_only_location` is `None`.
    /// Otherwise, `read_only_location` is the location of the simple command
    /// that executed the `readonly` built-in that made this variable read-only.
    pub read_only_location: Option<Location>,
}

impl Variable {
    /// Whether this variable is read-only or not.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.read_only_location.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VariableInContext {
    variable: Variable,
    context_index: usize,
}

/// Type of a context.
///
/// The context type affects the behavior of variable
/// [assignment](VariableSet::assign).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextType {
    /// Context for normal assignments.
    ///
    /// The base context is a regular context. The context for a function's
    /// local assignment is also regular.
    Regular,

    /// Context for temporary assignments.
    ///
    /// A volatile context is used for holding temporary variables when
    /// executing a built-in or function.
    Volatile,
}

/// Variable context.
///
/// Variables defined in the context are not stored in this struct.
/// See `VariableSet::all_variables`.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Context {
    /// Context type.
    r#type: ContextType,

    /// Positional parameters.
    ///
    /// This variable is very special:
    ///
    /// - Its value is always an `Array`.
    /// - It is never exported nor read-only.
    positional_params: Variable,
}

impl Context {
    fn new(r#type: ContextType) -> Self {
        Context {
            r#type,
            positional_params: Variable {
                value: Array(Vec::default()),
                last_assigned_location: None,
                is_exported: false,
                read_only_location: None,
            },
        }
    }
}

/// Collection of variables.
///
/// See the [module documentation](self) for details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariableSet {
    /// Hash map containing all variables.
    ///
    /// The value of a hash map entry is a stack of variables defined in
    /// contexts, sorted in the ascending order of the context index.
    ///
    /// Having the variables of all the contexts in this single hash map makes
    /// the variable search faster than having a separate hash map for each
    /// context.
    all_variables: HashMap<String, Vec<VariableInContext>>,

    /// Stack of contexts.
    ///
    /// The stack can never be empty since the base context is always the first
    /// item.
    contexts: Vec<Context>,
}

impl Default for VariableSet {
    fn default() -> Self {
        VariableSet {
            all_variables: Default::default(),
            contexts: vec![Context::new(ContextType::Regular)],
        }
    }
}

/// Choice of a context to which a variable is assigned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// Assigns to as lower a context as possible.
    ///
    /// If there is an existing variable in a [regular](ContextType::Regular)
    /// context, the variable is overwritten by the assignment. Existing
    /// [volatile](ContextType::Volatile) variables are removed to make the
    /// target variable visible.
    ///
    /// If there is no variable to overwrite, the assignment adds a new variable
    /// to the base context.
    Global,

    /// Assigns to the topmost regular context.
    ///
    /// Any existing variables below the topmost [regular](ContextType::Regular)
    /// context do not affect this type of assignment. Existing variables above
    /// the topmost regular context are removed.
    ///
    /// If the `VariableSet` only has the base context, the variable is assigned
    /// to it anyway.
    Local,

    /// Assigns to the topmost volatile context.
    ///
    /// This type of assignment requires the topmost context to be
    /// [volatile](ContextType::Volatile), or the assignment would **panic!**
    ///
    /// If an existing read-only variable would fail a `Global` assignment, the
    /// `Volatile` assignment fails for the same reason.
    Volatile,
}

// TODO Rename to AssignReadOnlyError
// TODO Add UnsetReadOnlyError that does not have the new_value field
/// Error that occurs when assigning to an existing read-only variable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlyError {
    /// Variable name.
    pub name: String,
    /// Location where the existing variable was made read-only.
    pub read_only_location: Location,
    /// New variable that was tried to assign.
    pub new_value: Variable,
}

impl std::fmt::Display for ReadOnlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "variable `{}` is read-only", self.name)
    }
}

impl std::error::Error for ReadOnlyError {}

impl VariableSet {
    /// Creates an empty variable set.
    #[must_use]
    pub fn new() -> VariableSet {
        Default::default()
    }

    /// Gets a reference to the variable with the specified name.
    ///
    /// You cannot retrieve positional parameters using this function.
    /// See [`positional_params`](Self::positional_params).
    #[must_use]
    pub fn get<N: ?Sized>(&self, name: &N) -> Option<&Variable>
    where
        String: Borrow<N>,
        N: Hash + Eq,
    {
        Some(&self.all_variables.get(name)?.last()?.variable)
    }

    /// Assigns a variable.
    ///
    /// If successful, the return value is the previous value. If there is an
    /// existing read-only value, the assignment fails unless the new variable
    /// is a local variable that hides the read-only.
    ///
    /// Note that this function does not return variables that it removed from
    /// volatile contexts. (See [`Scope`] for the conditions in which volatile
    /// variables are removed.)
    ///
    /// The current implementation assumes that variables in volatile contexts
    /// are not read-only.
    ///
    /// You cannot modify positional parameters using this function.
    /// See [`positional_params_mut`](Self::positional_params_mut).
    pub fn assign(
        &mut self,
        scope: Scope,
        name: String,
        mut value: Variable,
    ) -> Result<Option<Variable>, ReadOnlyError> {
        use std::collections::hash_map::Entry;
        // TODO Can we avoid cloning the name here?
        let stack = match self.all_variables.entry(name.clone()) {
            Entry::Vacant(vacant) => vacant.insert(Vec::new()),
            Entry::Occupied(occupied) => occupied.into_mut(),
        };

        // Volatile assignment cannot hide a read-only variable.
        if scope == Scope::Volatile {
            if let Some(vic) = stack.last() {
                if let Some(location) = &vic.variable.read_only_location {
                    return Err(ReadOnlyError {
                        name,
                        read_only_location: location.clone(),
                        new_value: value,
                    });
                }
            }
        }

        // To which context should we assign?
        let contexts = &self.contexts;
        let context_index = match scope {
            Scope::Global => stack
                .iter()
                .filter(|vic| contexts[vic.context_index].r#type != ContextType::Volatile)
                .next_back()
                .map_or(0, |vic| vic.context_index),
            Scope::Local => contexts
                .iter()
                .rposition(|c| c.r#type == ContextType::Regular)
                .expect("base context has gone"),
            Scope::Volatile => {
                let top_context = contexts.last().expect("base context has gone");
                assert_eq!(
                    top_context.r#type,
                    ContextType::Volatile,
                    "volatile scope assignment requires volatile context"
                );
                contexts.len() - 1
            }
        };

        // Remove volatile variables.
        while stack
            .last()
            .filter(|vic| vic.context_index > context_index)
            .is_some()
        {
            stack.pop();
        }

        // Do the assignment.
        let existing = stack
            .last_mut()
            .filter(|vic| vic.context_index == context_index)
            .map(|vic| &mut vic.variable);
        if let Some(existing) = existing {
            if let Some(location) = &existing.read_only_location {
                return Err(ReadOnlyError {
                    name,
                    read_only_location: location.clone(),
                    new_value: value,
                });
            }

            value.is_exported |= existing.is_exported;
            Ok(Some(std::mem::replace(existing, value)))
        } else {
            stack.push(VariableInContext {
                variable: value,
                context_index,
            });
            Ok(None)
        }
    }

    /// Returns environment variables in a new vector of C string.
    #[must_use]
    pub fn env_c_strings(&self) -> Vec<CString> {
        self.all_variables
            .iter()
            .filter_map(|(name, vars)| {
                let var = &vars.last()?.variable;
                if var.is_exported {
                    let mut s = name.clone();
                    s.push('=');
                    match &var.value {
                        Scalar(value) => s.push_str(value),
                        Array(values) => write!(s, "{}", values.iter().format(":")).ok()?,
                    }
                    // TODO return something rather than dropping null-containing strings
                    CString::new(s).ok()
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns a reference to the positional parameters.
    ///
    /// Every regular context starts with an empty array of positional
    /// parameters, and volatile contexts cannot have positional parameters.
    /// This function returns a reference to the positional parameters of the
    /// topmost regular context.
    ///
    /// See also [`positional_params_mut`](Self::positional_params_mut).
    #[must_use]
    pub fn positional_params(&self) -> &Variable {
        &self
            .contexts
            .iter()
            .filter(|c| c.r#type == ContextType::Regular)
            .last()
            .expect("base context has gone")
            .positional_params
    }

    /// Returns a mutable reference to the positional parameters.
    ///
    /// Although positional parameters are not considered a variable in the
    /// POSIX standard, we implement them as an anonymous array variable. It is
    /// the caller's responsibility to keep the variable in a correct state:
    ///
    /// - The variable value should be an array. Not a scalar.
    /// - The variable should not be exported nor made read-only.
    ///
    /// The `VariableSet` does not check if these rules are maintained.
    ///
    /// Every regular context starts with an empty array of positional
    /// parameters, and volatile contexts cannot have positional parameters.
    /// This function returns a reference to the positional parameters of the
    /// topmost regular context.
    #[must_use]
    pub fn positional_params_mut(&mut self) -> &mut Variable {
        &mut self
            .contexts
            .iter_mut()
            .filter(|c| c.r#type == ContextType::Regular)
            .last()
            .expect("base context has gone")
            .positional_params
    }

    fn push_context_impl(&mut self, context_type: ContextType) {
        self.contexts.push(Context::new(context_type));
    }

    fn pop_context_impl(&mut self) {
        debug_assert!(!self.contexts.is_empty());
        assert_ne!(self.contexts.len(), 1, "cannot pop the base context");
        self.contexts.pop();
        // TODO Use HashMap::drain_filter to remove empty values
        // TODO Use complementary stack of hash tables to avoid scanning the
        // whole `self.all_variables`
        for stack in self.all_variables.values_mut() {
            if let Some(vic) = stack.last() {
                if vic.context_index >= self.contexts.len() {
                    stack.pop();
                }
            }
        }
    }
}

/// RAII-style guard for temporarily retaining a variable context.
///
/// The guard object is created by [`VariableSet::push_context`].
#[derive(Debug)]
#[must_use = "You must retain ContextGuard to keep the context alive"]
pub struct ContextGuard<'a> {
    stack: &'a mut VariableSet,
}

impl VariableSet {
    /// Pushes a new empty context to this variable set.
    ///
    /// This function returns a scope guard that will pop the context when dropped.
    #[inline]
    pub fn push_context(&mut self, context_type: ContextType) -> ContextGuard<'_> {
        self.push_context_impl(context_type);
        ContextGuard { stack: self }
    }

    /// Pops the topmost context from the variable set.
    #[inline]
    pub fn pop_context(guard: ContextGuard<'_>) {
        drop(guard)
    }
}

impl std::ops::Drop for ContextGuard<'_> {
    /// Drops the `ContextGuard`.
    ///
    /// This function [pops](VariableSet::pop_context) the context that was
    /// pushed when creating this `ContextGuard`.
    #[inline]
    fn drop(&mut self) {
        self.stack.pop_context_impl()
    }
}

impl std::ops::Deref for ContextGuard<'_> {
    type Target = VariableSet;
    #[inline]
    fn deref(&self) -> &VariableSet {
        self.stack
    }
}

impl std::ops::DerefMut for ContextGuard<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut VariableSet {
        self.stack
    }
}

/// RAII-style guard that makes sure a context is popped properly
///
/// The guard object is created by [`Env::push_context`].
#[derive(Debug)]
#[must_use = "The context is popped when the guard is dropped"]
pub struct EnvContextGuard<'a> {
    env: &'a mut Env,
}

impl Env {
    /// Pushes a new context to the variable set.
    ///
    /// This function is equivalent to
    /// `self.variables.push_context(context_type)`, but returns an
    /// `EnvContextGuard` that allows re-borrowing the `Env`.
    #[inline]
    pub fn push_context(&mut self, context_type: ContextType) -> EnvContextGuard<'_> {
        self.variables.push_context_impl(context_type);
        EnvContextGuard { env: self }
    }

    /// Pops the topmost context from the variable set.
    #[inline]
    pub fn pop_context(guard: EnvContextGuard<'_>) {
        drop(guard)
    }
}

/// When the guard is dropped, the context that was pushed when creating the
/// guard is popped.
impl Drop for EnvContextGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        self.env.variables.pop_context_impl()
    }
}

impl Deref for EnvContextGuard<'_> {
    type Target = Env;
    #[inline]
    fn deref(&self) -> &Env {
        self.env
    }
}

impl DerefMut for EnvContextGuard<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Env {
        self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn assign_new_variable_and_get() {
        let mut variables = VariableSet::new();
        let variable = Variable {
            value: Scalar("my value".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: Some(Location::dummy("dummy")),
        };
        let result = variables
            .assign(Scope::Global, "foo".to_string(), variable.clone())
            .unwrap();
        assert_eq!(result, None);
        assert_eq!(variables.get("foo"), Some(&variable));
    }

    #[test]
    fn reassign_variable_and_get() {
        let mut variables = VariableSet::new();
        let v1 = Variable {
            value: Scalar("my value".to_string()),
            last_assigned_location: Some(Location::dummy("dummy")),
            is_exported: false,
            read_only_location: None,
        };
        variables
            .assign(Scope::Global, "foo".to_string(), v1.clone())
            .unwrap();

        let v2 = Variable {
            value: Scalar("your value".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: Some(Location::dummy("something")),
        };
        let result = variables
            .assign(Scope::Global, "foo".to_string(), v2.clone())
            .unwrap();
        assert_eq!(result, Some(v1));
        assert_eq!(variables.get("foo"), Some(&v2));
    }

    #[test]
    fn assign_to_read_only_variable() {
        let mut variables = VariableSet::new();
        let read_only_location = Location::dummy("read-only");
        let v1 = Variable {
            value: Scalar("my value".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: Some(read_only_location.clone()),
        };
        variables
            .assign(Scope::Global, "x".to_string(), v1.clone())
            .unwrap();

        let v2 = Variable {
            value: Scalar("your value".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: Some(Location::dummy("something")),
        };
        let error = variables
            .assign(Scope::Global, "x".to_string(), v2.clone())
            .unwrap_err();
        assert_eq!(error.name, "x");
        assert_eq!(error.read_only_location, read_only_location);
        assert_eq!(error.new_value, v2);
        assert_eq!(variables.get("x"), Some(&v1));
    }

    fn dummy_variable<V: Into<String>>(value: V) -> Variable {
        Variable {
            value: Scalar(value.into()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: None,
        }
    }

    #[test]
    fn assign_global() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Global, "foo".to_string(), dummy_variable(""))
            .unwrap();
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("".to_string()));
    }

    #[test]
    fn assign_local() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable(""))
            .unwrap();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("".to_string()));
    }

    #[test]
    fn popping_context_removes_variables() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable(""))
            .unwrap();
        variables.pop_context_impl();
        assert_eq!(variables.get("foo"), None);
    }

    #[test]
    fn reassign_global_non_base_context() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable("a"))
            .unwrap();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Global, "foo".to_string(), dummy_variable("b"))
            .unwrap();
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("b".to_string()));
        variables.pop_context_impl();
        assert_eq!(variables.get("foo"), None);
    }

    #[test]
    fn variable_in_upper_context_hides_lower_variables() {
        let mut variables = VariableSet::new();
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable("0"))
            .unwrap();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable("1"))
            .unwrap();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("1".to_string()));
    }

    #[test]
    fn variable_is_visible_again_after_popping_upper_variables() {
        let mut variables = VariableSet::new();
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable("0"))
            .unwrap();
        variables.push_context_impl(ContextType::Regular);
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable("1"))
            .unwrap();
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("0".to_string()));
    }

    #[test]
    fn volatile_assignment_new() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("0"))
            .unwrap();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("0".to_string()));
    }

    #[test]
    fn volatile_assignment_hides_existing_variable() {
        let mut variables = VariableSet::new();
        variables
            .assign(Scope::Global, "foo".to_string(), dummy_variable("0"))
            .unwrap();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("1"))
            .unwrap();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("1".to_string()));
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("0".to_string()));
    }

    #[test]
    fn volatile_assignment_fails_with_existing_read_only_variable() {
        let mut variables = VariableSet::new();
        let read_only_location = Location::dummy("ROL");
        let mut read_only = dummy_variable("0");
        read_only.read_only_location = Some(read_only_location.clone());
        variables
            .assign(Scope::Global, "foo".to_string(), read_only)
            .unwrap();
        variables.push_context_impl(ContextType::Volatile);
        let error = variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("1"))
            .unwrap_err();
        assert_eq!(error.name, "foo");
        assert_eq!(error.read_only_location, read_only_location);
        assert_eq!(error.new_value.value, Value::Scalar("1".to_string()));
    }

    #[test]
    #[should_panic(expected = "volatile scope assignment requires volatile context")]
    fn volatile_assignment_panics_without_volatile_context() {
        let mut variables = VariableSet::new();
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("0"))
            .unwrap();
    }

    #[test]
    fn global_assignment_pops_existing_volatile_variables() {
        let mut variables = VariableSet::new();
        variables
            .assign(Scope::Global, "foo".to_string(), dummy_variable("0"))
            .unwrap();
        variables.push_context_impl(ContextType::Regular);
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("1"))
            .unwrap();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("2"))
            .unwrap();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Global, "foo".to_string(), dummy_variable("9"))
            .unwrap();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("9".to_string()));
        variables.pop_context_impl();
        variables.pop_context_impl();
        variables.pop_context_impl();
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("9".to_string()));
    }

    #[test]
    fn local_assignment_pops_existing_volatile_variables() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("0"))
            .unwrap();
        variables.push_context_impl(ContextType::Regular);
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("1"))
            .unwrap();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Volatile, "foo".to_string(), dummy_variable("2"))
            .unwrap();
        variables.push_context_impl(ContextType::Volatile);
        variables
            .assign(Scope::Local, "foo".to_string(), dummy_variable("9"))
            .unwrap();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("9".to_string()));
        variables.pop_context_impl();
        variables.pop_context_impl();
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("9".to_string()));
        variables.pop_context_impl();
        let variable = variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("0".to_string()));
        variables.pop_context_impl();
        assert_eq!(variables.get("foo"), None);
    }

    #[test]
    #[should_panic(expected = "cannot pop the base context")]
    fn cannot_pop_base_context() {
        let mut variables = VariableSet::new();
        variables.pop_context_impl();
    }

    #[test]
    fn exporting() {
        let mut variables = VariableSet::new();
        let variable = Variable {
            value: Scalar("first".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: None,
        };
        variables
            .assign(Scope::Local, "foo".to_string(), variable)
            .unwrap();
        let variable = Variable {
            value: Scalar("second".to_string()),
            last_assigned_location: None,
            is_exported: true,
            read_only_location: None,
        };
        let old_value = variables
            .assign(Scope::Local, "foo".to_string(), variable)
            .unwrap()
            .unwrap();
        assert_eq!(old_value.value, Scalar("first".to_string()));
        assert!(!old_value.is_exported);
        let new_value = variables.get("foo").unwrap();
        assert_eq!(new_value.value, Scalar("second".to_string()));
        assert!(new_value.is_exported);
    }

    #[test]
    fn reexport_on_reassigning_exported_variable() {
        let mut variables = VariableSet::new();
        let variable = Variable {
            value: Scalar("first".to_string()),
            last_assigned_location: None,
            is_exported: true,
            read_only_location: None,
        };
        variables
            .assign(Scope::Local, "foo".to_string(), variable)
            .unwrap();
        let variable = Variable {
            value: Scalar("second".to_string()),
            last_assigned_location: None,
            is_exported: false,
            read_only_location: None,
        };
        let old_value = variables
            .assign(Scope::Local, "foo".to_string(), variable)
            .unwrap()
            .unwrap();
        assert_eq!(old_value.value, Scalar("first".to_string()));
        assert!(old_value.is_exported);
        let new_value = variables.get("foo").unwrap();
        assert_eq!(new_value.value, Scalar("second".to_string()));
        assert!(new_value.is_exported);
    }

    #[test]
    fn env_c_strings() {
        let mut variables = VariableSet::new();
        assert_eq!(&variables.env_c_strings(), &[]);

        variables
            .assign(
                Scope::Global,
                "foo".to_string(),
                Variable {
                    value: Scalar("FOO".to_string()),
                    last_assigned_location: None,
                    is_exported: true,
                    read_only_location: None,
                },
            )
            .unwrap();
        variables
            .assign(
                Scope::Global,
                "bar".to_string(),
                Variable {
                    value: Array(vec!["BAR".to_string()]),
                    last_assigned_location: None,
                    is_exported: true,
                    read_only_location: None,
                },
            )
            .unwrap();
        variables
            .assign(
                Scope::Global,
                "baz".to_string(),
                Variable {
                    value: Array(vec!["1".to_string(), "two".to_string(), "3".to_string()]),
                    last_assigned_location: None,
                    is_exported: true,
                    read_only_location: None,
                },
            )
            .unwrap();
        variables
            .assign(
                Scope::Global,
                "null".to_string(),
                Variable {
                    value: Scalar("not exported".to_string()),
                    last_assigned_location: None,
                    is_exported: false,
                    read_only_location: None,
                },
            )
            .unwrap();
        let mut ss = variables.env_c_strings();
        ss.sort_unstable();
        assert_eq!(
            &ss,
            &[
                CString::new("bar=BAR").unwrap(),
                CString::new("baz=1:two:3").unwrap(),
                CString::new("foo=FOO").unwrap()
            ]
        );
    }

    #[test]
    fn positional_params_in_base_context() {
        let mut variables = VariableSet::new();
        assert_eq!(variables.positional_params().value, Array(vec![]));

        let v = variables.positional_params_mut();
        assert_matches!(&mut v.value, Array(values) => {
            values.push("foo".to_string());
            values.push("bar".to_string());
        });

        assert_matches!(&variables.positional_params().value, Array(values) => {
            assert_eq!(values.as_ref(), ["foo".to_string(), "bar".to_string()]);
        });
    }

    #[test]
    fn positional_params_in_second_regular_context() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Regular);
        assert_eq!(variables.positional_params().value, Array(vec![]));

        let v = variables.positional_params_mut();
        assert_matches!(&mut v.value, Array(values) => {
            values.push("1".to_string());
        });

        assert_matches!(&variables.positional_params().value, Array(values) => {
            assert_eq!(values.as_ref(), ["1".to_string()]);
        });
    }

    #[test]
    fn getting_positional_params_in_volatile_context() {
        let mut variables = VariableSet::new();

        let v = variables.positional_params_mut();
        assert_matches!(&mut v.value, Array(values) => {
            values.push("a".to_string());
            values.push("b".to_string());
            values.push("c".to_string());
        });

        variables.push_context_impl(ContextType::Volatile);
        assert_matches!(&variables.positional_params().value, Array(values) => {
            assert_eq!(values.as_ref(), ["a".to_string(), "b".to_string(), "c".to_string()]);
        });
    }

    #[test]
    fn setting_positional_params_in_volatile_context() {
        let mut variables = VariableSet::new();
        variables.push_context_impl(ContextType::Volatile);

        let v = variables.positional_params_mut();
        assert_matches!(&mut v.value, Array(values) => {
            values.push("x".to_string());
        });

        variables.pop_context_impl();
        assert_matches!(&variables.positional_params().value, Array(values) => {
            assert_eq!(values.as_ref(), ["x".to_string()]);
        });
    }

    #[test]
    fn scope_guard() {
        let mut env = Env::new_virtual();
        let mut guard = env.variables.push_context(ContextType::Regular);
        guard
            .assign(Scope::Global, "foo".to_string(), dummy_variable(""))
            .unwrap();
        guard
            .assign(Scope::Local, "bar".to_string(), dummy_variable(""))
            .unwrap();
        VariableSet::pop_context(guard);

        let variable = env.variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("".to_string()));
        assert_eq!(env.variables.get("bar"), None);
    }

    #[test]
    fn env_scope_guard() {
        let mut env = Env::new_virtual();
        let mut guard = env.push_context(ContextType::Regular);
        guard
            .variables
            .assign(Scope::Global, "foo".to_string(), dummy_variable(""))
            .unwrap();
        guard
            .variables
            .assign(Scope::Local, "bar".to_string(), dummy_variable(""))
            .unwrap();
        Env::pop_context(guard);

        let variable = env.variables.get("foo").unwrap();
        assert_eq!(variable.value, Scalar("".to_string()));
        assert_eq!(env.variables.get("bar"), None);
    }
}
