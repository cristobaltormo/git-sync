use std::sync::Arc;
use std::time::Duration;

use super::notify::{EVENT_FAILING, Event};
use super::queue::Job;
use super::{Engine, full_name, unix_now};
use crate::error::Error;
use crate::forge::Repo;

const BACKOFF: [Duration; 4] = [
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
    Duration::from_secs(1800),
];

const BLOCKED_DELAY: Duration = Duration::from_secs(600);

impl Engine {
    pub(super) fn run_job(&self, job: &Job) -> Result<(), Error> {
        let repo: Arc<Repo> = match &job.repo {
            Some(r) => r.clone(),
            None => match self.src.get(&job.owner, &job.name)? {
                Some(r) => r,
                None => {
                    infof!(
                        "{} is gone from the source, checking",
                        full_name(&job.owner, &job.name)
                    );
                    self.wake();
                    return Ok(());
                }
            },
        };
        let (ok, why) = self.selected(&repo);
        if !ok {
            let newly = self.state.with(|st| {
                st.repos
                    .get(&repo.id)
                    .is_some_and(|x| !x.excluded && x.managed)
            });
            if newly {
                self.state.update(repo.id, false, |x| x.excluded = true);
                self.state.save();
                infof!(
                    "{}: no longer synced ({why}); the GitHub copy is left alone",
                    repo.full()
                );
            }
            return Ok(());
        }
        let res = self.sync_repo(&repo, job);
        if res.is_err() && self.vanished(&repo) {
            infof!(
                "{} was removed from the source while it was being synced",
                repo.full()
            );
            self.wake();
            return Ok(());
        }
        res
    }

    fn vanished(&self, r: &Repo) -> bool {
        matches!(self.src.get(&r.owner, &r.name), Ok(None))
    }

    pub(super) fn record_failure(&self, job: &Job, err: &Error) {
        let (id, name) = match &job.repo {
            Some(r) => (r.id, r.full()),
            None => (
                job.key.parse::<i64>().unwrap_or(0),
                full_name(&job.owner, &job.name),
            ),
        };
        let blocked = matches!(err, Error::Blocked(_));
        let busy = is_busy(err);
        if id == 0 {
            errorf!("{name}: {err}");
            return;
        }
        let msg = err.to_string();
        let mut attempts = 0;
        let now = unix_now();
        self.state.update(id, job.repo.is_some(), |x| {
            if x.owner.is_empty()
                && let Some(r) = &job.repo
            {
                x.owner.clone_from(&r.owner);
                x.name.clone_from(&r.name);
            }
            x.last_error.clone_from(&msg);
            x.attempts += 1;
            attempts = x.attempts;
            let delay = if busy {
                let shift = (x.attempts - 1).clamp(0, 3) as u32;
                (Duration::from_secs(10) * (1 << shift)).min(Duration::from_secs(60))
            } else if blocked {
                BLOCKED_DELAY
            } else {
                BACKOFF[((x.attempts - 1).max(0) as usize).min(BACKOFF.len() - 1)]
            };
            x.retry_at = now + delay.as_secs() as i64;
        });
        if attempts == 3 && !busy {
            self.notify(Event {
                kind: EVENT_FAILING,
                repo: name.clone(),
                info: msg,
            });
        }
        if busy {
            warnf!(
                "{name}: GitHub is still busy with a previous change, will retry shortly ({err})"
            );
        } else if blocked && attempts > 1 {
            debugf!("{name}: {err}");
        } else if blocked {
            warnf!("{name}: {err}");
        } else {
            errorf!("{name}: {err}");
        }
        self.state.save();
    }
}

pub(super) fn is_busy(err: &Error) -> bool {
    if let Error::Api { status: 422, msg } = err
        && msg.to_lowercase().contains("in progress")
    {
        return true;
    }
    err.to_string().contains("is disabled")
}
