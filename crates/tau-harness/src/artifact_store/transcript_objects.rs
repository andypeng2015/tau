//! Private hardlink aliases keep transcript bytes safe from older CAS cleaners.
//!
//! Older binaries only clean `blake3`, `operations`, and `.cleanup`. They never
//! modify published data in place, so an ordinary object detach cannot destroy
//! this alias. These are the same objects/keys, not another public namespace.

use super::transcript_roots::{path_present, read_regular, require_directory};
use super::*;

#[cfg(test)]
mod tests;

impl ArtifactStore {
    /// Mirrors a new explicit put's age into any private alias, never on reads.
    pub(super) fn renew_transcript_object(
        &self,
        descriptor: &ArtifactDescriptor,
        put_at: u64,
    ) -> Result<(), ArtifactError> {
        let digest = artifact_digest(&descriptor.key).ok_or(ArtifactError::Invalid)?;
        let directory = self.root.join("transcript-objects").join(digest);
        if !path_present(&directory)? {
            return Ok(());
        }
        require_directory(&directory)?;
        let mut metadata: Metadata = read_json(&directory.join("meta.json"))?;
        if metadata.version != 1 || metadata.size != descriptor.size.get() {
            return Err(ArtifactError::Integrity);
        }
        metadata.last_put_at = metadata.last_put_at.max(put_at);
        replace_json(&directory.join("meta.json"), &metadata)
    }

    /// Selects the ordinary object first, using its protected alias only if
    /// absent.
    ///
    /// Corrupt ordinary metadata is an error, not authority to silently repair
    /// or overwrite it. Reads do not republish an expired ordinary catalog
    /// entry.
    pub(super) fn object_directory(&self, digest: &str) -> Result<PathBuf, ArtifactError> {
        let ordinary = self.root.join("blake3").join(digest);
        if path_present(&ordinary)? {
            return Ok(ordinary);
        }
        Ok(self.root.join("transcript-objects").join(digest))
    }

    /// Publishes and verifies a hardlink alias under the caller's exclusive
    /// lock.
    ///
    /// This must finish before root publication. Failure may leave an unrooted
    /// alias, but cannot acknowledge a root with only old-cleaner-vulnerable
    /// bytes.
    pub(super) fn protect_transcript_object(
        &self,
        descriptor: &ArtifactDescriptor,
    ) -> Result<(), ArtifactError> {
        let digest = artifact_digest(&descriptor.key).ok_or(ArtifactError::Invalid)?;
        let objects = self.root.join("transcript-objects");
        private_dir(&objects)?;
        require_directory(&objects)?;
        let protected = objects.join(digest);
        if path_present(&protected)? {
            require_directory(&protected)?;
            let metadata: Metadata = read_json(&protected.join("meta.json"))?;
            if metadata.version != 1 || metadata.size != descriptor.size.get() {
                return Err(ArtifactError::Integrity);
            }
            verify(
                descriptor,
                &read_regular(&protected.join("data"), ARTIFACT_MAX_BYTES)?,
            )?;
        } else {
            let ordinary = self.root.join("blake3").join(digest);
            require_directory(&ordinary)?;
            verify(
                descriptor,
                &read_regular(&ordinary.join("data"), ARTIFACT_MAX_BYTES)?,
            )?;
            let staging = tempfile::Builder::new()
                .prefix(".staging-")
                .tempdir_in(&objects)
                .map_err(io_error)?;
            fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))
                .map_err(io_error)?;
            // Hardlink failure rejects admission; never silently copy or weaken
            // durability. Both paths are in the same selected CAS filesystem.
            fs::hard_link(ordinary.join("data"), staging.path().join("data")).map_err(io_error)?;
            File::open(staging.path().join("data"))
                .and_then(|file| file.sync_all())
                .map_err(io_error)?;
            write_json(
                &staging.path().join("meta.json"),
                &self.metadata(&descriptor.key)?,
            )?;
            sync_dir(staging.path())?;
            fs::rename(staging.path(), &protected).map_err(io_error)?;
        }
        // Retried publication repairs any earlier parent-sync failure.
        sync_dir(&protected)?;
        sync_dir(&objects)?;
        sync_dir(&self.root)
    }
}
