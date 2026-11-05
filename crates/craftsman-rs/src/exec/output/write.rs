//! Guard every actual finite-record IO boundary. No stdout/TTY/public schema.
use super::path::{Identity, identity};
use super::{CAP, Error, Target};
use crate::exec::terminal::{Record, Terminal};
use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::fs::{Dir, File};
use std::io::{Read, Seek, SeekFrom, Write};

const STEP: usize = 16 * 1024;
const CALLS: usize = 4096;
const DATA: &str = "record";
const TEMP: &str = ".craftsman-exec-tmp-";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    BeforeCreate,
    AfterCreate,
    BeforeOpen,
    AfterOpen,
    BeforeWrite,
    AfterWrite,
    BeforeFlush,
    AfterFlush,
    BeforeSync,
    AfterSync,
    BeforeCheck,
    AfterCheck,
    BeforeLink,
    AfterLink,
    BeforeCleanup,
    AfterCleanup,
    Finished,
}
#[derive(Default)]
pub(in crate::exec) struct Signals<'a> {
    #[cfg(not(test))]
    unused: std::marker::PhantomData<&'a ()>,
    #[cfg(test)]
    pub(super) hook: Option<&'a mut dyn FnMut(Event, &Terminal, &Dir)>,
    #[cfg(test)]
    pub(super) max_write: Option<usize>,
    #[cfg(test)]
    pub(super) max_read: Option<usize>,
    #[cfg(test)]
    pub(super) read_calls: usize,
    #[cfg(test)]
    pub(super) interrupted: usize,
    #[cfg(test)]
    pub(super) fail_write: Option<std::io::ErrorKind>,
    #[cfg(test)]
    pub(super) fail_flush: bool,
    #[cfg(test)]
    pub(super) fail_sync: bool,
}
impl Signals<'_> {
    fn event(&mut self, event: Event, terminal: &Terminal, directory: &Dir) {
        #[cfg(test)]
        if let Some(hook) = &mut self.hook {
            hook(event, terminal, directory);
        }
        #[cfg(not(test))]
        let _ = (event, terminal, directory);
    }
    fn write(&mut self, file: &mut File, bytes: &[u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        {
            if self.interrupted > 0 {
                self.interrupted -= 1;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            if let Some(error) = self.fail_write {
                return Err(error.into());
            }
            if let Some(maximum) = self.max_write {
                return file.write(&bytes[..bytes.len().min(maximum)]);
            }
        }
        file.write(bytes)
    }
    fn read(&mut self, file: &mut File, bytes: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        {
            self.read_calls += 1;
            if let Some(maximum) = self.max_read {
                let length = bytes.len().min(maximum);
                return file.read(&mut bytes[..length]);
            }
        }
        file.read(bytes)
    }
    fn flush(&mut self, file: &mut File) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_flush {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        file.flush()
    }
    fn sync(&mut self, file: &File) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_sync {
            return Err(std::io::ErrorKind::Other.into());
        }
        file.sync_all()
    }
}
fn guarded<T>(record: &Record<'_>, action: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    record.ready()?;
    let result = action();
    // Same epoch/lease wins even if the native action returned an IO failure.
    record.ready()?;
    result
}
struct Staging {
    parent: Dir,
    name: String,
    directory: Option<Dir>,
    directory_id: Identity,
    file: Option<File>,
    file_id: Identity,
    removed: bool,
}
impl Staging {
    fn directory(&self) -> Result<&Dir, Error> {
        self.directory.as_ref().ok_or(Error::Changed)
    }
    fn file(&self) -> Result<&File, Error> {
        self.file.as_ref().ok_or(Error::Changed)
    }
    fn file_mut(&mut self) -> Result<&mut File, Error> {
        self.file.as_mut().ok_or(Error::Changed)
    }
    fn create(
        target: &Target,
        terminal: &Terminal,
        record: &Record<'_>,
        signals: &mut Signals<'_>,
    ) -> Result<Self, Error> {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
        let name = format!(
            "{TEMP}{}",
            nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        signals.event(Event::BeforeCreate, terminal, &target.directory);
        guarded(record, || {
            target.validate()?;
            target.absent()?;
            Ok(())
        })?;
        record.ready()?;
        let created = crate::platform_os::confined_create_dir(&target.directory, &name);
        if let Err(error) = created {
            record.ready()?;
            return Err(error.into());
        }
        // Own directory is random/create-only/private on Unix. Do not recurse
        // or retry an existing entry. Windows retains inherited-ACL limits.
        let directory = target.directory.open_dir_nofollow(&name)?;
        let directory_id = identity(&directory.dir_metadata()?);
        signals.event(Event::AfterCreate, terminal, &directory);
        let result = (|| {
            record.ready()?;
            signals.event(Event::BeforeOpen, terminal, &directory);
            record.ready()?;
            let opened =
                directory.open_with(DATA, &crate::platform_os::confined_file_options(true, true));
            // Ensure cleanup owns the created file even if a post-open stop wins.
            let file = opened?;
            let file_id = identity(&file.metadata()?);
            let staging = Self {
                parent: target.directory.try_clone()?,
                name: name.clone(),
                directory: Some(directory),
                directory_id,
                file: Some(file),
                file_id,
                removed: false,
            };
            signals.event(Event::AfterOpen, terminal, staging.directory()?);
            record.ready()?;
            staging.validate(false)?;
            Ok::<_, Error>(staging)
        })();
        // Before a file exists we can remove only the original, still-empty
        // private directory. A replaced/nonempty entry is never recursively erased.
        if result.is_err() {
            if let Ok(dir) = target.directory.open_dir_nofollow(&name) {
                if dir
                    .dir_metadata()
                    .ok()
                    .is_some_and(|m| identity(&m) == directory_id)
                {
                    drop(dir);
                    let _ = target.directory.remove_dir(&name);
                }
            }
        }
        result
    }
    fn validate(&self, linked: bool) -> Result<(), Error> {
        let directory = self
            .parent
            .open_dir_nofollow(&self.name)
            .map_err(|_| Error::Changed)?;
        if identity(&directory.dir_metadata()?) != self.directory_id {
            return Err(Error::Changed);
        }
        let file = self
            .directory()?
            .open_with(
                DATA,
                &crate::platform_os::confined_file_options(false, false),
            )
            .map_err(|_| Error::Changed)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || identity(&metadata) != self.file_id
            || metadata.nlink() != if linked { 2 } else { 1 }
            || metadata.len() > CAP as u64
        {
            return Err(Error::Changed);
        }
        Ok(())
    }
    fn remove(&mut self) -> Result<(), Error> {
        let file = self
            .directory()?
            .open_with(
                DATA,
                &crate::platform_os::confined_file_options(false, false),
            )
            .map_err(|_| Error::Changed)?;
        if identity(&file.metadata()?) != self.file_id {
            return Err(Error::Changed);
        }
        drop(file);
        // Close original staging-path file/directory handles before removal:
        // Windows can otherwise retain delete-pending entries. No weaker
        // ownership check, recursive cleanup, final-name deletion or retry.
        drop(self.file.take());
        self.directory()?.remove_file(DATA)?;
        let dir = self
            .parent
            .open_dir_nofollow(&self.name)
            .map_err(|_| Error::Changed)?;
        if identity(&dir.dir_metadata()?) != self.directory_id {
            return Err(Error::Changed);
        }
        drop(dir);
        drop(self.directory.take());
        self.parent.remove_dir(&self.name)?;
        self.removed = true;
        Ok(())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.remove();
        }
    }
}
fn check_content(
    staging: &mut Staging,
    record: &Record<'_>,
    signals: &mut Signals<'_>,
) -> Result<(), Error> {
    guarded(record, || {
        staging
            .file_mut()?
            .seek(SeekFrom::Start(0))
            .map(|_| ())
            .map_err(Into::into)
    })?;
    let bytes = record.bytes()?;
    let mut offset = 0;
    // The successful native seek consumes one call in this pass; payload
    // reads (including Interrupted) and the final EOF probe share the rest.
    let mut calls = 1;
    let mut buffer = [0u8; STEP];
    while offset < bytes.len() {
        if calls >= CALLS {
            return Err(Error::Limit);
        }
        calls += 1;
        let length = (bytes.len() - offset).min(STEP);
        let read = guarded(record, || {
            signals
                .read(staging.file_mut()?, &mut buffer[..length])
                .map_err(Into::into)
        });
        match read {
            Err(Error::Io(std::io::ErrorKind::Interrupted)) => continue,
            Ok(0) => return Err(Error::Changed),
            Ok(count) => {
                if buffer[..count] != bytes[offset..offset + count] {
                    return Err(Error::Changed);
                }
                offset += count;
            }
            Err(e) => return Err(e),
        }
    }
    // Do not spend a 4097th call proving EOF after the payload reached the
    // ceiling. Refuse the entire checked record before any later pass/link.
    if calls >= CALLS {
        return Err(Error::Limit);
    }
    if guarded(record, || {
        signals
            .read(staging.file_mut()?, &mut buffer[..1])
            .map_err(Into::into)
    })? != 0
    {
        return Err(Error::Changed);
    }
    Ok(())
}
pub(super) fn publish(
    target: &Target,
    terminal: &Terminal,
    record: &Record<'_>,
    linked: &mut bool,
    signals: &mut Signals<'_>,
) -> Result<(), Error> {
    record.ready()?;
    let mut staging = Staging::create(target, terminal, record, signals)?;
    let bytes = record.bytes()?;
    let mut offset = 0;
    let mut calls = 0;
    while offset < bytes.len() {
        if calls >= CALLS {
            return Err(Error::Limit);
        }
        calls += 1;
        signals.event(Event::BeforeWrite, terminal, staging.directory()?);
        record.ready()?;
        let end = (offset + STEP).min(bytes.len());
        let result = signals.write(staging.file_mut()?, &bytes[offset..end]);
        signals.event(Event::AfterWrite, terminal, staging.directory()?);
        record.ready()?;
        match result {
            Ok(0) => return Err(Error::Io(std::io::ErrorKind::WriteZero)),
            Ok(n) if n <= end - offset => offset += n,
            Ok(_) => return Err(Error::Changed),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    signals.event(Event::BeforeFlush, terminal, staging.directory()?);
    record.ready()?;
    let flushed = signals.flush(staging.file_mut()?);
    signals.event(Event::AfterFlush, terminal, staging.directory()?);
    record.ready()?;
    flushed.map_err(Error::from)?;
    signals.event(Event::BeforeSync, terminal, staging.directory()?);
    record.ready()?;
    let synced = signals.sync(staging.file()?);
    signals.event(Event::AfterSync, terminal, staging.directory()?);
    record.ready()?;
    synced.map_err(Error::from)?;
    signals.event(Event::BeforeCheck, terminal, staging.directory()?);
    guarded(record, || staging.validate(false))?;
    check_content(&mut staging, record, signals)?;
    signals.event(Event::AfterCheck, terminal, staging.directory()?);
    record.ready()?;
    signals.event(Event::BeforeLink, terminal, staging.directory()?);
    guarded(record, || {
        target.validate()?;
        target.absent()?;
        staging.validate(false)
    })?;
    check_content(&mut staging, record, signals)?;
    record.ready()?;
    let result = staging
        .directory()?
        .hard_link(DATA, &target.directory, &target.name);
    *linked = result.is_ok();
    signals.event(Event::AfterLink, terminal, staging.directory()?);
    record.ready()?;
    result.map_err(Error::from)?;
    guarded(record, || staging.validate(true))?;
    check_content(&mut staging, record, signals)?;
    signals.event(Event::BeforeCleanup, terminal, staging.directory()?);
    record.ready()?;
    // Cleanup is deny-only and also runs on stop/error via Drop, without retrying
    // publication or deleting the final name. An already visible file can remain.
    let cleanup = staging.remove();
    signals.event(Event::AfterCleanup, terminal, &target.directory);
    record.ready()?;
    cleanup?;
    // Reopen ONLY the retained final directory's name after staging handles
    // close. Prove its original identity before reading, then retain it for
    // unchanged final content/EOF/namespace checks; never ambient/foreign IO.
    let final_file = guarded(record, || {
        target.validate()?;
        let file = target.directory.open_with(
            &target.name,
            &crate::platform_os::confined_file_options(false, false),
        )?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || identity(&metadata) != staging.file_id
            || metadata.nlink() != 1
            || metadata.len() != bytes.len() as u64
        {
            return Err(Error::Changed);
        }
        Ok(file)
    })?;
    staging.file = Some(final_file);
    check_content(&mut staging, record, signals)?;
    guarded(record, || {
        target.validate()?;
        let file = target.directory.open_with(
            &target.name,
            &crate::platform_os::confined_file_options(false, false),
        )?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || identity(&metadata) != staging.file_id
            || metadata.nlink() != 1
            || metadata.len() != bytes.len() as u64
        {
            return Err(Error::Changed);
        }
        Ok(())
    })?;
    signals.event(Event::Finished, terminal, &target.directory);
    record.ready()?;
    check_content(&mut staging, record, signals)?;
    guarded(record, || {
        target.validate()?;
        let file = target.directory.open_with(
            &target.name,
            &crate::platform_os::confined_file_options(false, false),
        )?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || identity(&metadata) != staging.file_id
            || metadata.nlink() != 1
            || metadata.len() != bytes.len() as u64
        {
            return Err(Error::Changed);
        }
        Ok(())
    })?;
    Ok(())
}
