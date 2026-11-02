pub trait LogHook: Send + Sync {}
pub struct NoLog;
impl LogHook for NoLog {}
