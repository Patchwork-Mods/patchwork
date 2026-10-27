//! Handing local mods' files to machines that don't have them.
//!
//! A local mod's file goes in the profile's document as `file/<hash>`,
//! `<hash>` being what it's locked as; iroh-docs then downloads it to every
//! machine that syncs the document. The store and iroh-blobs both hash
//! with BLAKE3, so a file's hash is the same to both. A folder mod is a
//! list of its files' paths at `file/<hash>`, and each of its files at
//! `file/<hash>/<path>`.
//!
//! Whatever arrives goes through [`Patchwork::add_synced_file`], which
//! refuses anything that isn't what was locked.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iroh_blobs::api::blobs::ImportMode;
use iroh_docs::api::Doc;
use iroh_docs::store::Query;
use patchwork_core::Patchwork;
use patchwork_core::lock::LockedMod;
use patchwork_core::profile::ProfileRef;
use patchwork_core::relpath::RelPath;
use patchwork_core::source::{ArtifactFormat, ArtifactLocation};
use patchwork_core::store::list_tree;
use patchwork_core::sync::FILE;
use serde::{Deserialize, Serialize};

use crate::{Node, doc_key, entry_key};

/// The files of a folder mod.
#[derive(Serialize, Deserialize)]
struct Manifest {
    files: Vec<String>,
}

/// Puts the files of `profile`'s local mods that this machine has, and the
/// document doesn't yet, in the document. Returns how many it put there.
pub(crate) async fn publish(
    node: &Node,
    pw: &Patchwork,
    doc: &Doc,
    profile: &ProfileRef,
) -> Result<usize> {
    let Some(lock) = pw.load_lock(profile)? else {
        return Ok(0);
    };
    let mut sent = 0;
    for m in lock.mods.iter().filter(|m| is_local(m)) {
        let Some(path) = pw.stored_file(m) else {
            continue;
        };
        let key = file_key(m);
        let there = keys_under(doc, &key).await?;
        if m.format != ArtifactFormat::Dir {
            if !there.contains(&key) {
                put_file(node, doc, &key, &path).await?;
                sent += 1;
            }
            continue;
        }
        let files = list_tree(&path)?;
        for (rel, src) in &files {
            let file_key = format!("{key}/{rel}");
            if !there.contains(&file_key) {
                put_file(node, doc, &file_key, src).await?;
            }
        }
        if !there.contains(&key) {
            let manifest = Manifest {
                files: files.iter().map(|(rel, _)| rel.to_string()).collect(),
            };
            doc.set_bytes(node.author(), doc_key(&key), serde_json::to_vec(&manifest)?)
                .await?;
            sent += 1;
        }
    }
    Ok(sent)
}

/// Puts the files of `profile`'s local mods that this machine is missing
/// into its store, from the document, as far as they've arrived. Returns
/// how many it got; ones that weren't what was locked are warned about.
pub(crate) async fn receive(
    node: &Node,
    pw: &Patchwork,
    doc: &Doc,
    profile: &ProfileRef,
    warnings: &mut Vec<String>,
) -> Result<usize> {
    let mut received = 0;
    for m in pw.missing_local_files(profile)? {
        let scratch = std::path::absolute(node.scratch().join(m.hash.to_string()))?;
        remove(&scratch)?;
        let got = if m.format == ArtifactFormat::Dir {
            receive_dir(node, doc, &m, &scratch).await?
        } else {
            take_file(node, doc, &file_key(&m), &scratch).await?
        };
        if got {
            match pw.add_synced_file(&m, &scratch) {
                Ok(()) => received += 1,
                Err(e) => warnings.push(e.to_string()),
            }
        }
        remove(&scratch)?;
    }
    Ok(received)
}

/// Writes a folder mod's files under `dest`, if they've all arrived.
async fn receive_dir(node: &Node, doc: &Doc, m: &LockedMod, dest: &Path) -> Result<bool> {
    let key = file_key(m);
    let Some(entry) = doc.get_one(Query::key_exact(doc_key(&key))).await? else {
        return Ok(false);
    };
    let hash = entry.content_hash();
    if !node.blobs().has(hash).await? && node.fetch(doc, hash).await.is_err() {
        return Ok(false);
    }
    let Ok(manifest) = node.blobs().get_bytes(hash).await else {
        return Ok(false);
    };
    let manifest: Manifest = serde_json::from_slice(&manifest)
        .with_context(|| format!("the file list for `{}` can't be read", m.name))?;
    for rel in manifest.files {
        // it came from another machine, so it mustn't point outside `dest`
        let rel = RelPath::new(&rel).map_err(anyhow::Error::msg)?;
        if !take_file(node, doc, &format!("{key}/{rel}"), &rel.under(dest)).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Copies the file at `key` to `dest`, fetching it if it hasn't arrived
/// (iroh-docs only tries once, when it first hears of it). `false` if it
/// isn't in the document or can't be fetched now.
async fn take_file(node: &Node, doc: &Doc, key: &str, dest: &Path) -> Result<bool> {
    let Some(entry) = doc.get_one(Query::key_exact(doc_key(key))).await? else {
        return Ok(false);
    };
    let hash = entry.content_hash();
    if !node.blobs().has(hash).await? && node.fetch(doc, hash).await.is_err() {
        return Ok(false);
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    node.blobs().export(hash, dest).finish().await?;
    Ok(true)
}

async fn put_file(node: &Node, doc: &Doc, key: &str, path: &Path) -> Result<()> {
    // the store never changes a file once it's in, so it needn't be copied
    let path = std::path::absolute(path)?;
    doc.import_file(
        node.blobs(),
        node.author(),
        doc_key(key).into(),
        path,
        ImportMode::TryReference,
    )
    .await?
    .await?;
    Ok(())
}

/// The keys in `doc` that are `key` or start with `key/`.
async fn keys_under(doc: &Doc, key: &str) -> Result<HashSet<String>> {
    let stream = doc.get_many(Query::key_prefix(key).build()).await?;
    let mut stream = std::pin::pin!(stream);
    let mut keys = HashSet::new();
    while let Some(entry) = stream.next().await {
        if let Some(k) = entry_key(entry?.key())
            && (k == key || k.starts_with(&format!("{key}/")))
        {
            keys.insert(k.to_owned());
        }
    }
    Ok(keys)
}

fn is_local(m: &LockedMod) -> bool {
    matches!(m.artifact, ArtifactLocation::Local { .. })
}

fn file_key(m: &LockedMod) -> String {
    format!("{FILE}{}", m.hash)
}

fn remove(path: &PathBuf) -> Result<()> {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| path.display().to_string())
        }
        _ => Ok(()),
    }
}
