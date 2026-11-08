//! Stopping a deploy partway, for frontends that offer to.
//!
//! A [`Cancel`] is shared between whatever's doing the work and whatever
//! might call it off. The work checks it between steps and stops with
//! [`Error::Cancelled`]; anything it's awaiting is dropped as soon as it's
//! cancelled, through [`Cancel::guard`].

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::future::{Either, select};
use tokio::sync::Notify;

use crate::error::{Error, Result};

#[derive(Clone, Default)]
pub struct Cancel(Arc<Inner>);

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancel {
    /// One that's never cancelled, for work nothing can call off.
    pub fn never() -> Self {
        Self::default()
    }

    /// Calls the work off. It stops at its next check.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Whether `other` is a clone of this one, rather than a different one.
    pub fn same(&self, other: &Cancel) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// [`Error::Cancelled`] once it's been cancelled.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Resolves once it's cancelled.
    pub async fn cancelled(&self) {
        loop {
            // Made before checking, so a cancel in between still wakes it.
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Runs `work`, dropping it at its next `.await` if this is cancelled
    /// first.
    pub async fn guard<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        self.check()?;
        let cancelled = pin!(self.cancelled());
        match select(pin!(work), cancelled).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(Error::Cancelled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guard_stops_pending_work() {
        let cancel = Cancel::default();
        let work = {
            let cancel = cancel.clone();
            async move {
                cancel.cancel();
                std::future::pending::<Result<()>>().await
            }
        };
        assert!(matches!(cancel.guard(work).await, Err(Error::Cancelled)));
    }

    #[tokio::test]
    async fn guard_passes_finished_work_through() {
        let cancel = Cancel::default();
        assert_eq!(cancel.guard(async { Ok(3) }).await.unwrap(), 3);
        cancel.cancel();
        assert!(cancel.check().is_err());
    }
}
