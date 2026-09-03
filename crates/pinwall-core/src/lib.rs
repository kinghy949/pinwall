//! PinWall 的平台无关核心逻辑。
//!
//! 本 crate 不依赖任何平台 API，全部行为均可脱离 UI 单测。

pub mod selection;

pub use selection::{Event, Outcome, Selection, SelectionMachine, SelectionPart, State};
