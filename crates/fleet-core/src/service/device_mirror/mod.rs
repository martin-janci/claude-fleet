//! Native scrcpy mirror of an Android device on a fleet host, reached
//! through a loopback relay. This module holds the adb wire preflight and
//! the local tool lookup; the orchestration lands in a later task.

pub mod adb;
pub mod tools;
