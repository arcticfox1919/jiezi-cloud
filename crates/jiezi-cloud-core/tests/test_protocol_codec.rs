use bytes::{BufMut, Bytes, BytesMut};
use jiezi_cloud_core::protocol::{
    frame_type, CodecError, Frame, FrameCodec, MAX_PAYLOAD_BYTES,
};
use jiezi_cloud_core::protocol::frames::{
    ByteRange, ChunkAckFrame, ChunkDataFrame, DownloadRequestFrame, ErrorCode, ErrorFrame,
    HelloAckFrame, HelloFrame, PingFrame, UploadAcceptFrame, UploadRequestFrame, JTP_VERSION,
};
use uuid::Uuid;

/// Round-trip helper: encode a frame, then decode it, assert equality.
fn round_trip(frame: Frame) -> Frame {
    let mut buf = BytesMut::new();
    FrameCodec::encode(&frame, &mut buf);
    FrameCodec::decode(&mut buf)
        .expect("decode should not error")
        .expect("should produce a frame")
}

#[test]
fn test_hello_round_trip() {
    let f = Frame::Hello(HelloFrame {
        version: JTP_VERSION,
        token: "eyJhbGciOiJFUzI1NiJ9.test".to_owned(),
    });
    let rt = round_trip(f);
    if let Frame::Hello(h) = rt {
        assert_eq!(h.version, JTP_VERSION);
        assert_eq!(h.token, "eyJhbGciOiJFUzI1NiJ9.test");
    } else {
        panic!("wrong frame type after round trip");
    }
}

#[test]
fn test_hello_ack_round_trip() {
    let f = Frame::HelloAck(HelloAckFrame { server_version: 1 });
    let rt = round_trip(f);
    if let Frame::HelloAck(h) = rt {
        assert_eq!(h.server_version, 1);
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_upload_request_round_trip() {
    let sid = Uuid::new_v4();
    let hash = [0xABu8; 32];
    let f = Frame::UploadRequest(UploadRequestFrame {
        session_id: sid,
        file_name: "hello world.mp4".to_owned(),
        total_size: 123_456_789,
        content_hash: hash,
        chunk_count: 42,
    });
    let rt = round_trip(f);
    if let Frame::UploadRequest(u) = rt {
        assert_eq!(u.session_id, sid);
        assert_eq!(u.file_name, "hello world.mp4");
        assert_eq!(u.total_size, 123_456_789);
        assert_eq!(u.content_hash, hash);
        assert_eq!(u.chunk_count, 42);
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_upload_accept_empty_missing() {
    let sid = Uuid::new_v4();
    let f = Frame::UploadAccept(UploadAcceptFrame {
        session_id: sid,
        missing_chunks: vec![],
        parallel_streams: 8,
        window_size: 16,
    });
    let rt = round_trip(f);
    if let Frame::UploadAccept(a) = rt {
        assert!(a.missing_chunks.is_empty());
        assert_eq!(a.parallel_streams, 8);
        assert_eq!(a.window_size, 16);
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_upload_accept_with_missing() {
    let sid = Uuid::new_v4();
    let f = Frame::UploadAccept(UploadAcceptFrame {
        session_id: sid,
        missing_chunks: vec![0, 3, 7, 99],
        parallel_streams: 4,
        window_size: 8,
    });
    let rt = round_trip(f);
    if let Frame::UploadAccept(a) = rt {
        assert_eq!(a.missing_chunks, vec![0, 3, 7, 99]);
        assert_eq!(a.parallel_streams, 4);
        assert_eq!(a.window_size, 8);
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_chunk_data_zero_copy() {
    let sid = Uuid::new_v4();
    let data = Bytes::from(vec![0x42u8; 1024]);
    let f = Frame::ChunkData(ChunkDataFrame {
        session_id: sid,
        chunk_index: 5,
        byte_offset: 5 * 1024,
        data: data.clone(),
    });
    let rt = round_trip(f);
    if let Frame::ChunkData(c) = rt {
        assert_eq!(c.chunk_index, 5);
        assert_eq!(c.byte_offset, 5120);
        assert_eq!(c.data, data);
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_chunk_ack_ok_and_fail() {
    for ok in [true, false] {
        let f = Frame::ChunkAck(ChunkAckFrame {
            session_id: Uuid::nil(),
            chunk_index: 0,
            ok,
        });
        let rt = round_trip(f);
        if let Frame::ChunkAck(a) = rt {
            assert_eq!(a.ok, ok);
        } else {
            panic!("wrong frame type");
        }
    }
}

#[test]
fn test_download_request_no_ranges() {
    let f = Frame::DownloadRequest(DownloadRequestFrame {
        session_id: Uuid::new_v4(),
        file_node_id: Uuid::new_v4(),
        byte_ranges: vec![],
    });
    let rt = round_trip(f);
    if let Frame::DownloadRequest(d) = rt {
        assert!(d.byte_ranges.is_empty());
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_download_request_with_ranges() {
    let f = Frame::DownloadRequest(DownloadRequestFrame {
        session_id: Uuid::new_v4(),
        file_node_id: Uuid::new_v4(),
        byte_ranges: vec![
            ByteRange { start: 0, end: 1023 },
            ByteRange { start: 2048, end: 4095 },
        ],
    });
    let rt = round_trip(f);
    if let Frame::DownloadRequest(d) = rt {
        assert_eq!(d.byte_ranges.len(), 2);
        assert_eq!(d.byte_ranges[1].start, 2048);
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_error_frame_nil_session() {
    let f = Frame::Error(ErrorFrame {
        session_id: Uuid::nil(),
        code: ErrorCode::Unauthorized,
        message: "token expired".to_owned(),
    });
    let rt = round_trip(f);
    if let Frame::Error(e) = rt {
        assert_eq!(e.session_id, Uuid::nil());
        assert_eq!(e.code, ErrorCode::Unauthorized);
        assert_eq!(e.message, "token expired");
    } else {
        panic!("wrong frame type");
    }
}

#[test]
fn test_ping_pong_round_trip() {
    let nonce: u64 = 0xDEAD_BEEF_CAFE_1234;
    for frame in [
        Frame::Ping(PingFrame { nonce }),
        Frame::Pong(PingFrame { nonce }),
    ] {
        let rt = round_trip(frame);
        let got_nonce = match &rt {
            Frame::Ping(p) | Frame::Pong(p) => p.nonce,
            _ => panic!("wrong frame type"),
        };
        assert_eq!(got_nonce, nonce);
    }
}

#[test]
fn test_partial_read_returns_none() {
    let frame = Frame::Hello(HelloFrame {
        version: JTP_VERSION,
        token: "abc".to_owned(),
    });
    let mut buf = BytesMut::new();
    FrameCodec::encode(&frame, &mut buf);

    // Only give the decoder the first 4 bytes (less than the 5-byte header).
    let partial = buf.split_to(4);
    let mut partial_buf = partial;
    let result = FrameCodec::decode(&mut partial_buf).expect("no error on partial");
    assert!(result.is_none(), "should return None on partial header");
}

#[test]
fn test_unknown_frame_type_errors() {
    let mut buf = BytesMut::new();
    buf.put_u8(0xEF); // unknown type
    buf.put_u32(0);   // zero-length payload
    let result = FrameCodec::decode(&mut buf);
    assert!(result.is_err());
}

#[test]
fn test_payload_too_large_rejected() {
    let mut buf = BytesMut::new();
    buf.put_u8(frame_type::PING);
    buf.put_u32(MAX_PAYLOAD_BYTES + 1);
    let result = FrameCodec::decode(&mut buf);
    assert!(matches!(result, Err(CodecError::PayloadTooLarge(_))));
}

#[test]
fn test_multiple_frames_in_one_buffer() {
    let mut buf = BytesMut::new();
    let f1 = Frame::Ping(PingFrame { nonce: 1 });
    let f2 = Frame::Pong(PingFrame { nonce: 2 });
    FrameCodec::encode(&f1, &mut buf);
    FrameCodec::encode(&f2, &mut buf);

    let r1 = FrameCodec::decode(&mut buf).unwrap().unwrap();
    let r2 = FrameCodec::decode(&mut buf).unwrap().unwrap();
    let r3 = FrameCodec::decode(&mut buf).unwrap();

    assert!(matches!(r1, Frame::Ping(_)));
    assert!(matches!(r2, Frame::Pong(_)));
    assert!(r3.is_none());
}
