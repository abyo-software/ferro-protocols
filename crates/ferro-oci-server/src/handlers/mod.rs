// SPDX-License-Identifier: Apache-2.0
//! HTTP handlers for the `/v2/**` OCI Distribution endpoints.
//!
//! Each submodule here corresponds to one family of endpoints in the
//! OCI Distribution Spec v1.1:
//!
//! - [`base`] — `GET /v2/` version check (spec §3.2);
//! - [`catalog`] — `GET /v2/_catalog` repository listing (spec §3.5);
//! - [`tags`] — `GET /v2/{name}/tags/list` (spec §3.6);
//! - [`blob`] — blob pull / delete (spec §3.2 / §4.9);
//! - [`blob_upload`] — chunked and monolithic pushes (spec §4.3–§4.8);
//! - [`manifest`] — manifest CRUD (spec §3.2 / §4.4 / §4.9);
//! - [`referrers`] — referrers API (spec §3.3).

pub mod base;
pub mod blob;
pub mod blob_upload;
pub mod catalog;
pub mod manifest;
pub mod referrers;
pub mod tags;
