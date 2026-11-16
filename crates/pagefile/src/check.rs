//! Checking a whole file: every page readable, every slotted page consistent,
//! and the freelist holding exactly the free pages.
//!
//! It cannot tell whether a slotted page or an overflow chain is still in use.
//! Only the code that stores data in the pages knows which pages it still
//! points to.
//!
//! It takes a `PageFile`, not any `PageStore`, because it checks the file as
//! it is on disk. That includes bytes past the last page, which only the
//! file can report.
//!
//! It finds these problems: a page that cannot be read or fails its
//! checksum, a slotted page whose slots do not add up, an unknown kind byte,
//! a freelist that reaches a page that is not free or loops, a free count
//! that does not match the freelist, a free page that is not on the freelist,
//! and bytes after the last page.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Error, PageFile, PageKind, PageNo, PageStore, Result, Slotted, freelist::FREE_NEXT};

/// What [`check`] found.
///
/// A file is healthy when `problems` is empty. `kinds` is there either
/// way, as a summary of what the file holds.
///
/// The problems are sentences, not error values, because the report is for
/// a person to read. Nothing reacts to one kind of problem differently from
/// another; the file is either healthy or it is not.
#[derive(Debug, Default)]
pub struct Report {
    /// How many pages there are of each kind, by name. Page 0 counts as
    /// "header", a page that cannot be read as "damaged", and a page with an
    /// unknown kind byte as "unknown".
    pub kinds: BTreeMap<&'static str, u32>,
    /// Everything wrong with the file, one sentence each. Empty for a healthy
    /// file.
    pub problems: Vec<String>,
}

/// Reads every page of `file` and walks its freelist, and reports what is
/// wrong. It reads the whole file, so it takes time in proportion to its
/// size. It never changes the file.
///
/// # Errors
///
/// Damaged pages are reported, not returned as errors. Only an
/// [`Error::Vfs`] stops the check, because then the file cannot be read at
/// all.
///
/// # Examples
///
/// ```
/// use pagefile::check::check;
/// use pagefile::{PageFile, freelist};
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// let n = freelist::allocate(&mut file).unwrap();
/// freelist::allocate(&mut file).unwrap();
/// freelist::free(&mut file, n).unwrap();
///
/// let report = check(&file).unwrap();
/// assert!(report.problems.is_empty());
/// assert_eq!(report.kinds["free"], 1);
/// assert_eq!(report.kinds["empty"], 1);
/// ```
pub fn check(file: &PageFile) -> Result<Report> {
    let mut report = Report::default();
    report.kinds.insert("header", 1);
    let mut page = file.new_page();
    let mut free_pages = BTreeSet::new();

    for number in 1..file.page_count() {
        let n = PageNo::new(number).expect("the loop starts at page 1");
        match file.read(n, &mut page) {
            Ok(()) => {}
            Err(Error::Vfs(e)) => return Err(Error::Vfs(e)),
            Err(e) => {
                report.problems.push(e.to_string());
                *report.kinds.entry("damaged").or_default() += 1;
                continue;
            }
        }
        let name = match page.kind() {
            Some(PageKind::Slotted) => {
                if let Err(e) = Slotted::open(&page) {
                    report.problems.push(format!("page {n}: {e}"));
                }
                "slotted"
            }
            Some(PageKind::Free) => {
                free_pages.insert(number);
                "free"
            }
            Some(kind) => kind.name(),
            None => {
                report.problems.push(format!(
                    "page {n} has unknown kind byte {}",
                    page.bytes()[0]
                ));
                "unknown"
            }
        };
        *report.kinds.entry(name).or_default() += 1;
    }

    // Walk the freelist. Each step must reach a free page not reached before.
    let mut on_list = BTreeSet::new();
    let mut next = file.header().free_head;
    while let Some(n) = next {
        if !free_pages.contains(&n.get()) {
            report.problems.push(format!(
                "the freelist reaches page {n}, which isn't a free page"
            ));
            break;
        }
        if !on_list.insert(n.get()) {
            report
                .problems
                .push(format!("the freelist loops back to page {n}"));
            break;
        }
        file.read(n, &mut page)?;
        next = PageNo::new(page.u32_at(FREE_NEXT));
    }
    let count = file.header().free_count;
    if on_list.len() as u32 != count {
        report.problems.push(format!(
            "the header counts {count} free pages, but the freelist holds {}",
            on_list.len()
        ));
    }
    for n in free_pages.difference(&on_list) {
        report
            .problems
            .push(format!("page {n} is free but not on the freelist"));
    }

    let expected = file.page_count() as u64 * file.page_size() as u64;
    let len = file.file_len()?;
    if len > expected {
        report.problems.push(format!(
            "the file has {} bytes after its last page",
            len - expected
        ));
    }
    Ok(report)
}
