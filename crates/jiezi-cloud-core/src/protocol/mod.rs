//! JTP/1 — Jiezi Transfer Protocol v1.
//!
//! This module contains:
//!
//! - [`frames`]    — all frame type definitions, wire constants and error codes.
//! - [`codec`]     — zero-copy binary encoder / decoder ([`FrameCodec`]).
//! - [`transport`] — [`JtpTransport`] trait + [`ChannelTransport`] for tests.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use bytes::BytesMut;
//! use jiezi_cloud_core::protocol::{Frame, FrameCodec};
//! use jiezi_cloud_core::protocol::frames::{HelloFrame, JTP_VERSION};
//!
//! // Encode a HELLO frame.
//! let mut buf = BytesMut::new();
//! FrameCodec::encode(
//!     &Frame::Hello(HelloFrame { version: JTP_VERSION, token: "my.jwt".into() }),
//!     &mut buf,
//! );
//!
//! // Decode it back.
//! let frame = FrameCodec::decode(&mut buf).unwrap().unwrap();
//! assert!(matches!(frame, Frame::Hello(_)));
//! ```

pub mod codec;
pub mod frames;
pub mod transport;

pub use codec::{CodecError, FrameCodec, MAX_PAYLOAD_BYTES};
pub use frames::{
    frame_type, ByteRange, ErrorCode, Frame, JTP_VERSION,
    ChunkAckFrame, ChunkDataFrame, CompleteAckFrame, CompleteFrame,
    DownloadCompleteFrame, DownloadInfoFrame, DownloadRequestFrame,
    ErrorFrame, HelloAckFrame, HelloFrame, PingFrame, ProgressFrame,
    UploadAcceptFrame, UploadInstantFrame, UploadRequestFrame, CancelFrame,
};
pub use transport::{ChannelTransport, JtpTransport};
