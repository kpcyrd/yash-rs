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

//! Type definitions for job management.

#[doc(no_inline)]
pub use nix::sys::wait::WaitStatus;
#[doc(no_inline)]
pub use nix::unistd::Pid;
use slab::Slab;
use std::iter::FusedIterator;

/// Set of one or more processes executing a pipeline
///
/// In the current implementation, a job contains the process ID of one child
/// process of the shell. Though there may be more processes involved in the
/// execution of the pipeline, the shell takes care of only one process of the
/// job.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Job {
    /// Process ID
    pub pid: Pid,

    /// Whether the job is job-controlled.
    ///
    /// If the job is job-controlled, the job process runs in its own process
    /// group.
    pub job_controlled: bool,

    /// Status of the process
    pub status: WaitStatus,

    /*
    pub status_changed: bool,
    */
    /// String representation of this process
    pub name: String,
    /*
    pub known_by_user: bool,
    */
}

impl Job {
    /// Creates a new job instance.
    ///
    /// This function requires a process ID to initialize the new job. The other
    /// members of the job are defaulted.
    pub fn new(pid: Pid) -> Self {
        Job {
            pid,
            job_controlled: false,
            status: WaitStatus::StillAlive,
            name: String::new(),
        }
    }
}

/// Iterator of jobs with indices.
///
/// Call [`JobSet::iter`] to get an instance of `Iter`.
#[derive(Clone, Debug)]
pub struct Iter<'a>(slab::Iter<'a, Job>);

impl<'a> Iterator for Iter<'a> {
    type Item = (usize, &'a Job);

    #[inline(always)]
    fn next(&mut self) -> Option<(usize, &'a Job)> {
        self.0.next()
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a> DoubleEndedIterator for Iter<'a> {
    #[inline(always)]
    fn next_back(&mut self) -> Option<(usize, &'a Job)> {
        self.0.next_back()
    }
}

impl ExactSizeIterator for Iter<'_> {
    #[inline(always)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

impl FusedIterator for Iter<'_> {}

/// Collection of jobs.
#[derive(Clone, Debug)]
pub struct JobSet {
    /// Jobs managed by the shell
    jobs: Slab<Job>,

    /// Process ID of the most recently executed asynchronous command.
    last_async_pid: Pid,
}

impl Default for JobSet {
    fn default() -> Self {
        JobSet {
            jobs: Slab::new(),
            last_async_pid: Pid::from_raw(0),
        }
    }
}

impl JobSet {
    /// Adds a job to this job set.
    ///
    /// This function returns a unique index assigned to the job.
    #[inline]
    pub fn add_job(&mut self, job: Job) -> usize {
        self.jobs.insert(job)
    }

    /// Removes a job from this job set.
    ///
    /// This function returns the job removed from the job set.
    /// The result is `None` if there is no job for the index.
    #[inline]
    pub fn remove_job(&mut self, index: usize) -> Option<Job> {
        self.jobs.try_remove(index)
    }

    /// Returns the job at the specified index.
    ///
    /// The result is `None` if there is no job for the index.
    #[inline]
    pub fn get_job(&self, index: usize) -> Option<&Job> {
        self.jobs.get(index)
    }

    /// Returns an iterator of jobs with indices.
    ///
    /// The item type of the returned iterator is `(usize, &Job)`.
    /// Jobs are iterated in the order of indices.
    #[inline]
    pub fn iter(&self) -> Iter {
        Iter(self.jobs.iter())
    }
}

impl JobSet {
    /// Returns the process ID of the most recently executed asynchronous
    /// command.
    ///
    /// This function returns the value that has been set by
    /// [`set_last_async_pid`](Self::set_last_async_pid), or 0 if no value has
    /// been set.
    ///
    /// When expanding the special parameter `$!`, you must use
    /// [`expand_last_async_pid`](Self::expand_last_async_pid) instead of this
    /// function.
    pub fn last_async_pid(&self) -> Pid {
        self.last_async_pid
    }

    /// Returns the process ID of the most recently executed asynchronous
    /// command.
    ///
    /// This function is similar to [`last_async_pid`](Self::last_async_pid),
    /// but also updates an internal flag so that the asynchronous command is
    /// not disowned too soon.
    ///
    /// TODO Elaborate on automatic disowning
    pub fn expand_last_async_pid(&mut self) -> Pid {
        // TODO Keep the async process from being disowned.
        self.last_async_pid
    }

    /// Sets the process ID of the most recently executed asynchronous command.
    ///
    /// This function affects the result of
    /// [`last_async_pid`](Self::last_async_pid).
    pub fn set_last_async_pid(&mut self, pid: Pid) {
        self.last_async_pid = pid;
    }
}
