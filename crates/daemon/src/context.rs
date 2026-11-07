use anyhow::Result;
use dtshim_core::model::Start;

use crate::policy::Policy;
use crate::volume::Volumes;

pub(crate) struct Ctx {
    pub(crate) policy: Policy,
    pub(crate) volumes: Volumes,
}

impl Ctx {
    pub(crate) fn prepare(&self, start: Start) -> Result<Start> {
        let start = self.volumes.map_start(start);
        self.policy.prepare(start)
    }
}
