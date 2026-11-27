use std::time::Duration;

use tokio::time::Instant;
use twilight_gateway::{ConfigBuilder, ShardId, queue::InMemoryQueue};
use twilight_http::Client;
use twilight_model::gateway::connection_info::BotConnectionInfo;

use super::{GatewaySession, GatewaySet};
use crate::{Error, PresenceHandle, Result};

pub(crate) enum GatewayPlan {
    Single,
    Recommended(Recommendation),
}

pub(crate) struct Recommendation {
    shards: u32,
    max_concurrency: u16,
    remaining: u32,
    reset_after: Duration,
    total: u32,
}

impl GatewayPlan {
    pub(crate) async fn discover(http: &Client, recommended: bool) -> Result<Self> {
        if !recommended {
            return Ok(Self::Single);
        }
        let info = http.gateway().authed().await?.model().await?;
        Ok(Self::Recommended(Recommendation::from_info(info)?))
    }

    pub(crate) fn prepare(
        self,
        config: ConfigBuilder,
        presence: Option<&PresenceHandle>,
    ) -> Result<GatewaySet> {
        let Self::Recommended(plan) = self else {
            return Ok(GatewaySet {
                sessions: vec![GatewaySession::new(ShardId::ONE, config, presence)],
                wave_size: None,
            });
        };
        let count =
            usize::try_from(plan.shards).map_err(|_| Error::InvalidGatewayRecommendation)?;
        let mut sessions = Vec::new();
        sessions
            .try_reserve_exact(count)
            .map_err(|_| Error::InvalidGatewayRecommendation)?;
        let queue = InMemoryQueue::new(
            plan.max_concurrency,
            plan.remaining,
            plan.reset_after,
            plan.total,
        );
        let config = config.queue(queue).build();
        for number in 0..plan.shards {
            let id = ShardId::new_checked(number, plan.shards)
                .ok_or(Error::InvalidGatewayRecommendation)?;
            sessions.push(GatewaySession::new(
                id,
                ConfigBuilder::from(config.clone()),
                presence,
            ));
        }
        Ok(GatewaySet {
            sessions,
            wave_size: Some(usize::from(plan.max_concurrency).min(count)),
        })
    }
}

impl Recommendation {
    fn from_info(info: BotConnectionInfo) -> Result<Self> {
        let limit = info.session_start_limit;
        let reset_after = Duration::from_millis(limit.reset_after);
        if info.shards == 0
            || limit.max_concurrency == 0
            || limit.total == 0
            || limit.remaining > limit.total
            || reset_after.is_zero()
            || Instant::now().checked_add(reset_after).is_none()
        {
            return Err(Error::InvalidGatewayRecommendation);
        }
        if limit.remaining < info.shards {
            return Err(Error::InsufficientGatewaySessions {
                required: info.shards,
                remaining: limit.remaining,
            });
        }
        Ok(Self {
            shards: info.shards,
            max_concurrency: limit.max_concurrency,
            remaining: limit.remaining,
            reset_after,
            total: limit.total,
        })
    }
}

#[cfg(test)]
mod tests {
    use twilight_model::gateway::SessionStartLimit;

    use super::*;

    fn info(
        shards: u32,
        max_concurrency: u16,
        remaining: u32,
        reset_after: u64,
    ) -> BotConnectionInfo {
        BotConnectionInfo {
            url: "wss://gateway.discord.gg".into(),
            shards,
            session_start_limit: SessionStartLimit {
                max_concurrency,
                remaining,
                reset_after,
                total: 10,
            },
        }
    }

    #[test]
    fn recommendations_reject_invalid_ranges_and_unsafe_session_budgets() {
        for invalid in [
            info(0, 2, 5, 20_000),
            info(2, 0, 5, 20_000),
            info(2, 2, 11, 20_000),
            info(2, 2, 5, 0),
        ] {
            assert!(matches!(
                Recommendation::from_info(invalid),
                Err(Error::InvalidGatewayRecommendation)
            ));
        }
        assert!(matches!(
            Recommendation::from_info(info(3, 2, 2, 20_000)),
            Err(Error::InsufficientGatewaySessions {
                required: 3,
                remaining: 2
            })
        ));
        assert!(Recommendation::from_info(info(2, 2, 2, 20_000)).is_ok());
    }
}
