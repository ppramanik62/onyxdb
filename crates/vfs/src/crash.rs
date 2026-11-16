//! `Mem::crash`: what a power failure keeps and what it loses.
//!
//! A crash is a power failure followed by a restart. Data and names that
//! were synced always survive. For every change after the last sync, the
//! seed decides what survives:
//!
//! ```text
//! a write     cut into 512-byte sectors; each sector is kept, lost, or torn
//!             (new bytes from one end of the sector, old bytes after them)
//! a set_len   kept or lost
//! names       the first k changes since the directory's last sync_dir,
//!             in order, for some k from 0 to all of them
//! ```
//!
//! These rules allow more results than most real disks and file systems
//! produce. A test may see a crash that a real disk would not cause, but it
//! will not miss one that a real disk can cause. That is the safer mistake
//! for a test to make.
//!
//! Names follow a stricter rule than bytes. The name changes in a directory
//! survive in order, with no gap: if a crash keeps one change, it keeps
//! every change before it. File systems write their name changes to their
//! own log in order, so a real crash never keeps a later rename and loses an
//! earlier one. If `Mem` allowed gaps, tests would report bugs that cannot
//! happen on a real disk.
//!
//! The power cut decides when the crash happens. `power_off_after(n)` lets
//! `n` more changes happen. Every change after that fails with
//! `Error::PowerOff`, as on a machine that has lost power.
//!
//! The cut is counted in changes, not in time, so it is a plain number. The
//! same `n` stops the same workload at the same change on every run. So a
//! test can try every point one by one: after the first change, after the
//! second, and so on.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    Error, Mem, Result, Rng,
    fault::Faults,
    mem::{Change, Dir, Inode, Node, State, write_into},
};

/// The size of a sector, the unit a disk writes in, in bytes. 512 is the
/// smallest sector disks use, so it gives the most ways to tear a write.
pub const SECTOR: u64 = 512;

impl Mem {
    /// How many changes have been made so far: writes, `set_len`s, syncs,
    /// and creating, removing and renaming files and directories. A change
    /// refused because the power is off does not count. A crash does not
    /// reset it.
    ///
    /// A test counts a workload's changes with this, then crashes it after
    /// each one in turn.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use vfs::{Mem, Open, Vfs};
    ///
    /// let mem = Mem::new();
    /// let file = mem.open(Path::new("a"), Open::CreateNew).unwrap();
    /// file.write_at(b"x", 0).unwrap();
    /// let mut buf = [0; 1];
    /// file.read_at(&mut buf, 0).unwrap();
    /// // The create and the write are changes. The read isn't.
    /// assert_eq!(mem.changes(), 2);
    /// ```
    pub fn changes(&self) -> u64 {
        self.lock().changes
    }

    /// Lets `n` more changes happen, then cuts the power. After that, every
    /// change fails with [`Error::PowerOff`] until `crash` is called. Reads
    /// still work. With `n` = 0 the power is off immediately.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use vfs::{Error, Mem, Open, Vfs};
    ///
    /// let mem = Mem::new();
    /// let file = mem.open(Path::new("a"), Open::CreateNew).unwrap();
    /// mem.power_off_after(1);
    /// file.write_at(b"one", 0).unwrap();
    /// assert!(matches!(file.write_at(b"two", 0), Err(Error::PowerOff)));
    /// ```
    pub fn power_off_after(&self, n: u64) {
        self.lock().power_left = Some(n);
    }
    /// Simulates a power failure and a restart. `seed` picks what survives, so
    /// the same seed always gives the same result. After the crash, everything
    /// left is synced, the power is on, no faults are waiting, every lock is
    /// released, and handles opened before the crash return [`Error::Stale`].
    ///
    /// It can be called with the power on or off. Calling it again crashes
    /// again, from what the last crash left.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use vfs::{Error, Mem, Open, Vfs};
    ///
    /// let mem = Mem::new();
    /// let file = mem.open(Path::new("a"), Open::CreateNew).unwrap();
    /// file.write_at(b"kept", 0).unwrap();
    /// file.sync().unwrap();
    /// mem.sync_dir(Path::new("")).unwrap();
    /// // Not synced, so the crash may keep it, lose it, or tear it.
    /// file.write_at(b"maybe", 4).unwrap();
    ///
    /// mem.crash(1);
    /// assert!(matches!(file.len(), Err(Error::Stale(_))));
    /// let file = mem.open(Path::new("a"), Open::Existing).unwrap();
    /// let mut buf = [0; 4];
    /// file.read_at(&mut buf, 0).unwrap();
    /// assert_eq!(&buf, b"kept");
    /// ```
    pub fn crash(&self, seed: u64) {
        let mut rng = Rng::new(seed);
        let mut state = self.lock();
        for inode in &mut state.files {
            inode.data = survivor(inode, &mut rng);
            inode.synced = inode.data.clone();
            inode.unsynced.clear();
        }
        for dir in state.dirs.values_mut() {
            let kept = rng.below(dir.unsynced.len() + 1);
            let mut names = dir.synced.clone();
            for change in &dir.unsynced[..kept] {
                change.apply(&mut names);
            }
            dir.names = names.clone();
            dir.synced = names;
            dir.unsynced.clear();
        }
        drop_unreachable(&mut state.dirs);
        state.generation += 1;
        state.power_left = None;
        state.faults = Faults::default();
        // The process that held them is gone.
        state.locked.clear();
    }
}

impl State {
    /// Counts one change, or refuses it if the power is off. Every change
    /// calls this before it does anything.
    ///
    /// # Errors
    ///
    /// [`Error::PowerOff`] if the power cut has been reached.
    pub(crate) fn change(&mut self) -> Result<()> {
        match self.power_left {
            Some(0) => return Err(Error::PowerOff),
            Some(n) => self.power_left = Some(n - 1),
            None => {}
        }
        self.changes += 1;
        Ok(())
    }
}

/// What `inode` holds after a crash. It starts from the synced bytes, and
/// `rng` decides whether each later change is kept, lost or torn. The kept
/// changes are applied in order, so a later write can overwrite part of an
/// earlier one.
fn survivor(inode: &Inode, rng: &mut Rng) -> Vec<u8> {
    let mut data = inode.synced.clone();
    for change in &inode.unsynced {
        match change {
            Change::SetLen(len) => {
                if rng.below(2) == 1 {
                    data.resize(*len as usize, 0);
                }
            }
            Change::Write { offset, bytes } => {
                let end = offset + bytes.len() as u64;
                let mut start = *offset;
                // One sector at a time: from `start` to the next sector
                // boundary or the end.
                while start < end {
                    let piece_end = ((start / SECTOR + 1) * SECTOR).min(end);
                    let piece = &bytes[(start - offset) as usize..(piece_end - offset) as usize];
                    let (from, to) = tear(piece.len(), rng);
                    if from < to {
                        write_into(&mut data, start + from as u64, &piece[from..to]);
                    }
                    start = piece_end;
                }
            }
        }
    }
    data
}

/// Which of a sector's `len` new bytes reach the disk: none, all, or part.
/// A part always starts at one end, because a disk writes a sector from one
/// end to the other.
fn tear(len: usize, rng: &mut Rng) -> (usize, usize) {
    match rng.below(4) {
        0 => (0, 0),
        1 => (0, len),
        2 => (0, rng.below(len + 1)),
        _ => (rng.below(len + 1), len),
    }
}

/// Removes directories whose own name did not survive the crash, and
/// everything below them.
fn drop_unreachable(dirs: &mut BTreeMap<PathBuf, Dir>) {
    // A directory sorts before everything inside it, so each parent is
    // handled before its children.
    let paths: Vec<PathBuf> = dirs.keys().cloned().collect();
    for path in paths {
        let Some(name) = path.file_name() else {
            continue;
        };
        let parent = path.parent().unwrap_or(Path::new(""));
        let named = dirs
            .get(parent)
            .is_some_and(|d| d.names.get(name) == Some(&Node::Dir));
        if !named {
            dirs.remove(&path);
        }
    }
}
