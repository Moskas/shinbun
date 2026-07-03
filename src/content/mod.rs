//! Feed content pipeline: HTML → Markdown → styled terminal lines.
//!
//! `html` converts feed-provided HTML into GitHub-flavored Markdown (tables,
//! images, links) in a single DOM walk, collecting outbound links as it goes.
//! `markdown` renders that Markdown into ratatui lines with real table layout
//! and structured image segments.

pub mod html;
pub mod markdown;
