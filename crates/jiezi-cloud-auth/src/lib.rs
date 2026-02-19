//! Authentication and authorization module for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::auth::AuthService`] using
//! Argon2 password hashing, JWT token management and an RBAC permission engine.

pub mod jwt;
pub mod password;
pub mod rbac;
pub mod repository;
pub mod service;
