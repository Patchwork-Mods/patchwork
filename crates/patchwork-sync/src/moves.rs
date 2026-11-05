//! Removing someone from a profile shared with friends.
//!
//! A write ticket holds its document's key, which can't be taken back, so
//! removing someone ([`Node::remove_peers`]) moves everyone else to a new
//! document they don't have the key to. Its ticket can't go through the
//! old document, which whoever's removed can still read, so it's handed
//! straight to each of the others, at the endpoint ids in the old
//! document's `peer/` keys, over [`ALPN`]. Whoever's told passes it on to
//! the rest too, in case whoever did the removing goes offline before
//! they're all told, and keeps trying those it can't reach yet.
//!
//! Every machine that moves puts [`MOVED`] in the old document as it
//! leaves. A machine that sees it stops taking changes from that document,
//! for good, as whoever's removed could still write to it. Followers then
//! need a new ticket: they're not in the `peer/` keys, so can't be told.
//!
//! Word of a move is only taken from a machine in the `peer/` keys of the
//! document the profile's synced through, for a document holding the same
//! profile. That doesn't stop one friend moving the others away from
//! another: anyone who can change the profile can remove anyone else.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::future::{BoxFuture, join_all};
use iroh::endpoint::{Connection, VarInt};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, PublicKey};
use iroh_docs::api::Doc;
use iroh_docs::api::protocol::ShareMode;
use iroh_docs::{CapabilityKind, DocTicket};
use patchwork_core::Patchwork;
use patchwork_core::profile::ProfileRef;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    Addresses, DEVICE_PROFILE, MOVED, Node, Outcome, PEER, SYNC_TIMEOUT, SyncedProfile, Watched,
    files, ticket, wait_for_sync,
};

/// The protocol a machine tells another where a profile's document moved
/// over.
pub(crate) const ALPN: &[u8] = b"patchwork/moved/0";

/// How long a running [`Node`] waits before trying again to tell those it
/// couldn't reach.
pub(crate) const TELL_EVERY: Duration = Duration::from_secs(60);

/// How long getting through to a machine to tell it can take, before
/// it's taken to be offline.
const CONNECT_WITHIN: Duration = Duration::from_secs(10);

/// How long telling a machine can take once through: it joining the new
/// document.
const TELL_WITHIN: Duration = Duration::from_secs(120);

/// How long a machine that's been told has to take it in, before whoever
/// told it gives up and tries again later.
const ANSWER_WITHIN: Duration = Duration::from_secs(90);

/// How long a machine stays in a document it's left after marking it
/// [`MOVED`], so others online then hear it.
const MOVED_GRACE: Duration = Duration::from_secs(3);

/// The most a notice or its answer can be.
const MAX_MESSAGE: usize = 64 * 1024;

/// Where a profile's document moved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Notice {
    profile: Uuid,
    /// The document it was synced through.
    from: String,
    /// A write ticket to the one it's synced through now.
    ticket: String,
}

/// What a machine that's told of a move says to it. Either way, it needn't
/// be told again; a machine that can't take it in yet doesn't answer.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "kebab-case")]
pub(crate) enum Answer {
    Moved,
    Refused { reason: String },
}

/// A notice from another machine, waiting for [`Node::run`] or
/// [`Node::sync_once`] to take it in.
pub(crate) struct Incoming {
    from: PublicKey,
    notice: Notice,
    answer: oneshot::Sender<Answer>,
}

/// What came of telling one machine.
pub(crate) struct Told {
    profile: Uuid,
    /// The document it was told about.
    doc: String,
    peer: PublicKey,
    answer: Result<Answer>,
}

/// What [`Node::remove_peers`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    /// How many of those staying were told where the profile went.
    pub told: usize,
    /// How many couldn't be reached yet. They're told once they and a
    /// machine syncing the profile are online at once.
    pub to_tell: usize,
}

/// Takes notices of moves from other machines.
#[derive(Debug, Clone)]
pub(crate) struct Handler {
    incoming: mpsc::UnboundedSender<Incoming>,
}

impl Handler {
    pub(crate) fn new(incoming: mpsc::UnboundedSender<Incoming>) -> Self {
        Self { incoming }
    }
}

impl ProtocolHandler for Handler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let from = conn.remote_id();
        let (mut send, mut recv) = conn.accept_bi().await.map_err(AcceptError::from_err)?;
        let bytes = recv
            .read_to_end(MAX_MESSAGE)
            .await
            .map_err(AcceptError::from_err)?;
        let Ok(notice) = serde_json::from_slice::<Notice>(&bytes) else {
            conn.close(VarInt::from_u32(1), b"that isn't a notice");
            return Ok(());
        };
        let (answer, answered) = oneshot::channel();
        let incoming = Incoming {
            from,
            notice,
            answer,
        };
        if self.incoming.send(incoming).is_err() {
            conn.close(VarInt::from_u32(2), b"shutting down");
            return Ok(());
        }
        // not answered means try again later
        let Ok(Ok(answer)) = tokio::time::timeout(ANSWER_WITHIN, answered).await else {
            conn.close(VarInt::from_u32(2), b"not now");
            return Ok(());
        };
        let answer = serde_json::to_vec(&answer).map_err(AcceptError::from_err)?;
        send.write_all(&answer)
            .await
            .map_err(AcceptError::from_err)?;
        send.finish()?;
        // for the answer to arrive before the connection goes
        conn.closed().await;
        Ok(())
    }
}

/// Tells the machine at `to` of a move.
async fn tell(endpoint: Endpoint, to: EndpointAddr, notice: Notice) -> Result<Answer> {
    let conn = tokio::time::timeout(CONNECT_WITHIN, endpoint.connect(to, ALPN))
        .await
        .context("couldn't get through")??;
    let attempt = async {
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(&serde_json::to_vec(&notice)?).await?;
        send.finish()?;
        let answer = recv.read_to_end(MAX_MESSAGE).await?;
        conn.close(VarInt::from_u32(0), b"thanks");
        anyhow::Ok(serde_json::from_slice(&answer)?)
    };
    tokio::time::timeout(TELL_WITHIN, attempt)
        .await
        .context("timed out")?
}

impl Node {
    /// The other machines, by endpoint id, that can change `profile`, as
    /// [`Node::remove_peers`] takes them. Followers aren't among them.
    pub async fn peers(&self, profile: &ProfileRef) -> Result<Vec<PublicKey>> {
        let Some(synced) = self.synced_profile(profile)? else {
            bail!("that profile isn't synced");
        };
        let me = self.id();
        let doc = self.open(&synced).await?;
        Ok(self
            .peers_in(&doc)
            .await?
            .into_iter()
            .filter(|p| !p.gone && p.id != me)
            .map(|p| p.id)
            .collect())
    }

    /// Stops the machines `peers` (endpoint ids, see [`Node::peers`])
    /// syncing `profile` with anyone else: it moves to a new document,
    /// which the others are told about, and the old one's left to whoever
    /// was removed. Followers stop getting changes too, and need a new
    /// ticket.
    pub async fn remove_peers(
        &self,
        pw: &Patchwork,
        profile: &ProfileRef,
        peers: &[PublicKey],
    ) -> Result<Removed> {
        let Some(synced) = self.synced_profile(profile)? else {
            bail!("that profile isn't synced");
        };
        if !synced.writable {
            bail!("this machine only follows the profile, so it can't remove anyone from it");
        }
        if synced.moved {
            bail!("the profile has moved somewhere this machine hasn't been told about yet");
        }
        let me = self.id();
        if peers.contains(&me) {
            bail!("that's this computer; leave the profile instead");
        }
        if peers.is_empty() {
            bail!("no one to remove");
        }
        let old = self.open(&synced).await?;
        let in_old = self.peers_in(&old).await?;
        for peer in peers {
            if !in_old.iter().any(|p| p.id == *peer && !p.gone) {
                bail!("{peer} isn't syncing this profile");
            }
        }
        // taking what's arrived, so nothing anyone else changed is lost
        self.sync_profile(pw, &synced).await?;

        let staying: Vec<_> = in_old
            .iter()
            .filter(|p| !p.gone && p.id != me && !peers.contains(&p.id))
            .collect();
        let new = self.keep(self.docs.create().await?).await?;
        let entries = pw.start_sync(profile)?;
        self.write(&new, entries.into_iter().collect()).await?;
        // so the ones staying can find each other, and are known to be in it
        let mut known = Vec::new();
        for p in &staying {
            known.push((format!("{PEER}{}", p.id), serde_json::to_vec(&p.addr())?));
        }
        self.write(&new, known).await?;
        self.announce(&new).await?;
        files::publish(self, pw, &new, profile).await?;
        let to_tell = staying.iter().map(|p| p.id.to_string()).collect();
        self.move_to(pw, &synced, &old, new, to_tell).await?;
        let moved = self
            .synced_profile(profile)?
            .context("the profile stopped being synced")?;
        self.tell_now(&moved).await
    }

    /// Switches `synced` from `old` to `new`, leaving `to_tell` to be told.
    async fn move_to(
        &self,
        pw: &Patchwork,
        synced: &SyncedProfile,
        old: &Doc,
        new: Doc,
        to_tell: Vec<String>,
    ) -> Result<Outcome> {
        let uuid = synced.profile;
        self.write(old, vec![(MOVED.to_owned(), b"{}".to_vec())])
            .await?;
        tokio::time::sleep(MOVED_GRACE).await;
        self.drop_doc(Watched::Profile(uuid), old.id()).await?;
        let moved = SyncedProfile {
            moved_from: Some(old.id().to_string()),
            to_tell,
            ..SyncedProfile::new(uuid, &new, true, true)
        };
        self.remember(moved.clone())?;
        self.watch(Watched::Profile(uuid), &new).await?;
        self.start(&new).await?;
        // so computers linked later get the new one
        if let Some(devices) = self.devices_doc().await? {
            let key = format!("{DEVICE_PROFILE}{uuid}");
            let ticket = ticket(&new, ShareMode::Write, Addresses::Included).await?;
            self.write(&devices, vec![(key, serde_json::to_vec(&ticket)?)])
                .await?;
        }
        self.sync_profile(pw, &moved).await
    }

    /// Takes in the notices that have come in, for [`Node::sync_once`].
    pub(crate) async fn take_moves(&self, pw: &Patchwork) -> Vec<Outcome> {
        let Some(mut rx) = self.moves_rx.lock().unwrap().take() else {
            return Vec::new();
        };
        let mut incoming = Vec::new();
        while let Ok(one) = rx.try_recv() {
            incoming.push(one);
        }
        *self.moves_rx.lock().unwrap() = Some(rx);
        let mut outcomes = Vec::new();
        for one in incoming {
            let profile = ProfileRef {
                uuid: one.notice.profile,
            };
            match self.take_move(pw, one).await {
                Some(Ok(outcome)) => outcomes.push(outcome),
                Some(Err(e)) => outcomes.push(Outcome {
                    warnings: vec![format!("{e:#}")],
                    ..Outcome::nothing(profile)
                }),
                None => {}
            }
        }
        outcomes
    }

    /// Takes in a notice, answering it. What syncing the profile then did,
    /// if it moved.
    pub(crate) async fn take_move(
        &self,
        pw: &Patchwork,
        incoming: Incoming,
    ) -> Option<Result<Outcome>> {
        let Incoming {
            from,
            notice,
            answer,
        } = incoming;
        match self.take_notice(pw, from, &notice).await {
            Ok((reply, outcome)) => {
                let _ = answer.send(reply);
                outcome.map(Ok)
            }
            // not answered, so it's sent again later
            Err(e) => Some(Err(e.context(format!(
                "couldn't move profile {} to its new document",
                notice.profile
            )))),
        }
    }

    async fn take_notice(
        &self,
        pw: &Patchwork,
        from: PublicKey,
        notice: &Notice,
    ) -> Result<(Answer, Option<Outcome>)> {
        let refuse = |reason: &str| {
            Ok((
                Answer::Refused {
                    reason: reason.to_owned(),
                },
                None,
            ))
        };
        let profile = ProfileRef {
            uuid: notice.profile,
        };
        let Some(synced) = self.synced_profile(&profile)? else {
            return refuse("that profile isn't synced here");
        };
        let Ok(ticket) = notice.ticket.parse::<DocTicket>() else {
            return refuse("that isn't a sync ticket");
        };
        let to = ticket.capability.id();
        if synced.doc == to.to_string() {
            return Ok((Answer::Moved, None));
        }
        if synced.moved_from.as_deref() == Some(notice.from.as_str()) {
            return refuse("it's already moved somewhere else");
        }
        if synced.doc != notice.from {
            return refuse("it isn't synced through that document here");
        }
        if !synced.writable {
            return refuse("this machine only follows it");
        }
        if !matches!(ticket.capability.kind(), CapabilityKind::Write) {
            return refuse("that ticket doesn't let it be changed");
        }
        let old = self.open(&synced).await?;
        let in_old = self.peers_in(&old).await?;
        if !in_old.iter().any(|p| p.id == from && !p.gone) {
            return refuse("whoever sent that isn't syncing it");
        }

        // it has to be the same profile before the old document's left
        let (new, events) = self.docs.import_and_subscribe(ticket).await?;
        let new = self.keep(new).await?;
        wait_for_sync(events, SYNC_TIMEOUT).await;
        let entries = self.read_entries(&new).await?;
        if entries.is_empty() {
            self.forget_doc(to).await?;
            bail!("nothing arrived from it within {}s", SYNC_TIMEOUT.as_secs());
        }
        let same = patchwork_core::sync::from_entries(&entries)
            .is_ok_and(|s| s.profile.reference() == profile);
        if !same {
            self.forget_doc(to).await?;
            return refuse("that document holds another profile");
        }
        let me = self.id();
        let to_tell = self
            .peers_in(&new)
            .await?
            .into_iter()
            .filter(|p| !p.gone && p.id != me && p.id != from)
            .map(|p| p.id.to_string())
            .collect();
        let outcome = self.move_to(pw, &synced, &old, new, to_tell).await?;
        Ok((Answer::Moved, Some(outcome)))
    }

    /// Notices for the machines `synced` still has to tell of its move, but
    /// isn't telling yet, each sent when it's awaited.
    pub(crate) async fn notices(
        &self,
        synced: &SyncedProfile,
    ) -> Result<Vec<BoxFuture<'static, Told>>> {
        let Some(from) = synced.moved_from.clone() else {
            return Ok(Vec::new());
        };
        let doc = self.open(synced).await?;
        let known = self.peers_in(&doc).await?;
        let ticket = ticket(&doc, ShareMode::Write, Addresses::Included).await?;
        let mut notices = Vec::new();
        for id in &synced.to_tell {
            let Ok(peer) = id.parse::<PublicKey>() else {
                continue;
            };
            if !self.telling.lock().unwrap().insert((synced.profile, peer)) {
                continue;
            }
            let to = known
                .iter()
                .find(|p| p.id == peer)
                .map(|p| p.addr())
                .unwrap_or_else(|| EndpointAddr::new(peer));
            let notice = Notice {
                profile: synced.profile,
                from: from.clone(),
                ticket: ticket.clone(),
            };
            let endpoint = self.router.endpoint().clone();
            let profile = synced.profile;
            let doc = synced.doc.clone();
            notices.push(Box::pin(async move {
                Told {
                    profile,
                    doc,
                    peer,
                    answer: tell(endpoint, to, notice).await,
                }
            }) as BoxFuture<'static, Told>);
        }
        Ok(notices)
    }

    /// Notes what came of telling a machine. One that answered, either
    /// way, needn't be told again.
    pub(crate) fn told(&self, told: Told) -> Result<()> {
        self.telling
            .lock()
            .unwrap()
            .remove(&(told.profile, told.peer));
        if told.answer.is_err() {
            return Ok(());
        }
        let Some(mut synced) = self.synced_profile(&ProfileRef { uuid: told.profile })? else {
            return Ok(());
        };
        if synced.doc != told.doc {
            return Ok(());
        }
        let peer = told.peer.to_string();
        synced.to_tell.retain(|id| *id != peer);
        self.remember(synced)
    }

    /// Tells the machines `synced` still has to tell of its move, waiting
    /// for each to answer.
    pub(crate) async fn tell_now(&self, synced: &SyncedProfile) -> Result<Removed> {
        let mut told = 0;
        for one in join_all(self.notices(synced).await?).await {
            if one.answer.is_ok() {
                told += 1;
            }
            self.told(one)?;
        }
        let to_tell = self
            .synced_profile(&synced.reference())?
            .map_or(0, |s| s.to_tell.len());
        Ok(Removed { told, to_tell })
    }
}
