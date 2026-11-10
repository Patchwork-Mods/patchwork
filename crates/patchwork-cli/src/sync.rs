//! `patchwork sync`: sharing profiles between computers.

use anyhow::{Result, bail};
use clap::Subcommand;
use patchwork_core::Patchwork;
use patchwork_sync::{Addresses, Joined, Node};

use crate::common::selected;

#[derive(Subcommand)]
pub(crate) enum SyncCommand {
    /// Link another of your own computers to this one: every profile on
    /// either, including ones made later, is kept in step on both. Prints a
    /// ticket to `join` with there, then keeps everything in step until
    /// stopped with Ctrl-C.
    Link,
    /// Stop linking this computer to your others. Profiles synced so far
    /// stay synced; stop each with `leave`.
    Unlink,
    /// Share the profile with friends, printing a ticket for them to
    /// `join` with, then keep it in step until stopped with Ctrl-C. Anyone
    /// with the ticket can add, remove and change mods, and every change
    /// goes to everyone.
    Share {
        /// Let others follow the profile instead: they get your mods and
        /// your changes, and can add mods of their own, which stay on their
        /// computer. Only you can change what everyone gets. Letting people
        /// change it can't be taken back once they have the ticket.
        #[arg(long)]
        read_only: bool,
        /// Leave this computer's IP addresses out of the ticket, so it can
        /// be posted publicly. Others reach it through its relay instead,
        /// and may still learn them once connected.
        #[arg(long)]
        hide_addresses: bool,
    },
    /// Join a profile someone shared, or link this computer to another of
    /// yours, with the ticket from `share` or `link`. The other computer
    /// has to be online. Mods are downloaded, but not deployed.
    Join { ticket: String },
    /// List the profiles this computer syncs.
    List,
    /// Stop syncing the profile here. It stays as it is, here and
    /// elsewhere, and linked computers no longer pick it up.
    Leave,
    /// List the other computers that can change the profile, by the id
    /// each prints with `patchwork sync id`, and the name you gave each.
    Peers,
    /// Give another computer a name to know it by here (see `peers`), or
    /// forget its name if none is given. Names aren't synced.
    Name { id: String, name: Vec<String> },
    /// Stop the computers with these ids (see `peers`) syncing the profile
    /// with anyone else. Everyone else moves to a new document, which
    /// they're told about when they're next online; whoever follows the
    /// profile needs a new ticket.
    Remove {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Keep every synced profile in step until stopped with Ctrl-C. Changes
    /// from other machines are saved and downloaded, never deployed.
    Run,
    /// Print the id other computers know this one by.
    Id,
    /// Give this computer a new id, e.g. if its old one got out alongside
    /// its addresses. Profiles stay synced, and the others are told the old
    /// id is gone the next time this one syncs. Tickets handed out before
    /// still work, but only while someone else syncing it is online.
    ResetId,
}

pub(crate) async fn run(
    pw: &Patchwork,
    command: Option<SyncCommand>,
    explicit: &Option<String>,
) -> Result<()> {
    if let Some(SyncCommand::Name { id, name }) = &command {
        let peer = id
            .parse()
            .map_err(|_| anyhow::anyhow!("`{id}` isn't a computer's id"))?;
        let name = name.join(" ");
        patchwork_sync::set_peer_name(&pw.paths().sync(), &peer, &name)?;
        if name.trim().is_empty() {
            println!("forgot the name of {id}");
        } else {
            println!("{id} is now called {}", name.trim());
        }
        return Ok(());
    }
    let node = Node::spawn(&pw.paths().sync()).await?;
    if matches!(command, Some(SyncCommand::ResetId)) {
        let old = node.id();
        let new = node.reset_id().await?;
        println!("{new}");
        eprintln!("this computer was {old}; others hear it's gone the next time it syncs");
        return Ok(());
    }
    let result = sync_with(pw, &node, command, explicit).await;
    node.shutdown().await?;
    result
}

async fn sync_with(
    pw: &Patchwork,
    node: &Node,
    command: Option<SyncCommand>,
    explicit: &Option<String>,
) -> Result<()> {
    let Some(command) = command else {
        if node.synced()?.is_empty() && !node.devices_linked()? {
            bail!(
                "no profiles are synced; link your computers with `patchwork sync link`, \
                 or share a profile with `patchwork sync share`"
            );
        }
        for outcome in node.sync_once(pw, patchwork_sync::SYNC_TIMEOUT).await? {
            print_sync(pw, &Ok(outcome), false);
        }
        return Ok(());
    };
    match command {
        SyncCommand::Link => {
            let ticket = node.link_devices(pw).await?;
            println!("{ticket}");
            eprintln!(
                "\nRun `patchwork sync join <ticket>` on your other computer with that ticket.\n\
                 It lets whoever has it change every profile here, so keep it to yourself.\n\
                 Keeping everything in step until Ctrl-C."
            );
            run_sync(pw, node).await?;
        }
        SyncCommand::Unlink => {
            node.unlink_devices().await?;
            println!(
                "unlinked; profiles synced so far stay synced (`patchwork sync leave` stops one)"
            );
        }
        SyncCommand::Share {
            read_only,
            hide_addresses,
        } => {
            let (profile, loaded) = selected(pw, explicit)?;
            let addresses = if hide_addresses {
                Addresses::Hidden
            } else {
                Addresses::Included
            };
            let ticket = node.share(pw, &profile, !read_only, addresses).await?;
            println!("{ticket}");
            let who = if read_only {
                "Whoever joins with it can follow the profile, but not change it for anyone else."
            } else {
                "Whoever joins with it can change the profile for everyone, which can't be taken back."
            };
            eprintln!(
                "\nGive that ticket to whoever's joining, to run `patchwork sync join <ticket>`.\n\
                 {who}\nKeeping {} in step until Ctrl-C.",
                loaded.label()
            );
            run_sync(pw, node).await?;
        }
        SyncCommand::Join { ticket } => {
            eprintln!("waiting for the other computer...");
            match node.join(pw, &ticket, JOIN_TIMEOUT).await? {
                Joined::Profile(outcome) => {
                    let loaded = pw.load_profile(&outcome.profile)?;
                    print_sync(pw, &Ok(outcome), false);
                    println!(
                        "joined {}; `patchwork sync` keeps it in step, and `patchwork -p {} deploy` deploys it",
                        loaded.label(),
                        loaded.label()
                    );
                }
                Joined::Devices { profiles } => {
                    eprintln!(
                        "linked; fetching {profiles} profile(s) from your other computers..."
                    );
                    for outcome in node.sync_once(pw, patchwork_sync::SYNC_TIMEOUT).await? {
                        print_sync(pw, &Ok(outcome), false);
                    }
                    println!(
                        "every profile is now kept in step with your other computers; \
                         `patchwork sync` or `patchwork sync run` keeps them so"
                    );
                }
            }
        }
        SyncCommand::List => {
            if node.devices_linked()? {
                println!("linked to your other computers: every profile is synced");
            }
            for synced in node.synced()? {
                let profile = synced.reference();
                let label = pw
                    .load_profile(&profile)
                    .map(|p| p.label())
                    .unwrap_or_else(|_| synced.profile.to_string());
                if synced.writable {
                    println!("{label}");
                    continue;
                }
                let unshared = pw.unshared_mods(&profile).unwrap_or_default();
                if unshared.is_empty() {
                    println!("{label} (following)");
                } else {
                    println!("{label} (following; only here: {})", unshared.join(", "));
                }
            }
        }
        SyncCommand::Leave => {
            let (profile, loaded) = selected(pw, explicit)?;
            node.leave(pw, &profile).await?;
            println!("stopped syncing {}", loaded.label());
        }
        SyncCommand::Peers => {
            let (profile, _) = selected(pw, explicit)?;
            let names = node.peer_names()?;
            for peer in node.peers(&profile).await? {
                match names.get(&peer.to_string()) {
                    Some(name) => println!("{peer}  {name}"),
                    None => println!("{peer}"),
                }
            }
        }
        SyncCommand::Remove { ids } => {
            let (profile, loaded) = selected(pw, explicit)?;
            let peers = ids
                .iter()
                .map(|id| {
                    id.parse()
                        .map_err(|_| anyhow::anyhow!("`{id}` isn't a computer's id"))
                })
                .collect::<Result<Vec<_>>>()?;
            eprintln!("moving everyone else to a new document...");
            let removed = node.remove_peers(pw, &profile, &peers).await?;
            println!(
                "removed {} from {}; told {} of the others",
                ids.join(", "),
                loaded.label(),
                removed.told
            );
            if removed.to_tell > 0 {
                eprintln!(
                    "{} couldn't be reached yet; they're told when they and a computer \
                     syncing {} are next online (`patchwork daemon install` keeps this one trying)",
                    removed.to_tell,
                    loaded.label()
                );
            }
        }
        SyncCommand::Run => run_sync(pw, node).await?,
        SyncCommand::Id => println!("{}", node.id()),
        SyncCommand::ResetId | SyncCommand::Name { .. } => unreachable!("handled by `sync`"),
    }
    Ok(())
}

/// How long `sync join` waits for the profile to arrive.
const JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

async fn run_sync(pw: &Patchwork, node: &Node) -> Result<()> {
    tokio::select! {
        result = node.run(pw, |outcome| print_sync(pw, &outcome, true)) => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

/// Says what a sync did. Running, only changes are worth a mention.
fn print_sync(pw: &Patchwork, outcome: &Result<patchwork_sync::Outcome>, running: bool) {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("warning: sync failed: {e:#}");
            return;
        }
    };
    let label = pw
        .load_profile(&outcome.profile)
        .map(|p| p.label())
        .unwrap_or_else(|_| outcome.profile.to_string());
    match &outcome.applied {
        _ if outcome.moved => println!(
            "{label}: moved to a new document; another computer syncing it hands that over \
             when you're both online, unless you were removed or only follow it, \
             in which case you need a new ticket"
        ),
        Some(applied) if applied.created => println!("{label}: added"),
        Some(applied) if applied.changed => println!("{label}: updated"),
        _ if running => {}
        None => println!("{label}: nothing from other computers yet"),
        Some(_) => println!("{label}: up to date"),
    }
    if outcome.received_files > 0 {
        println!(
            "{label}: got {} local mod file(s) from other computers",
            outcome.received_files
        );
    }
    for warning in &outcome.warnings {
        eprintln!("warning: {label}: {warning}");
    }
}
