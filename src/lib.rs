//! ccusage-rs's library half: the transcript parser, the rules for what a
//! usage record bills, and the one price table. Every estate tool that puts
//! dollars on Claude Code usage reads them from here, so two reports never
//! disagree about a price.

pub mod parse;
pub mod pricing;
pub mod usage;
