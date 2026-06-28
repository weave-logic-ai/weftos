//! CBOR wire-format round-trip on every `SceneOp` variant.
//!
//! Each variant is encoded, decoded, and compared structurally. This
//! is the canonical wire-stability test; if you change a wire type,
//! a corresponding test failure here is the expected blast radius.

use weftos_leaf_scene::{
    codec::{decode_input_envelope, decode_scene_envelope, encode, CodecError},
    geometry::{px, Point, Rect, Transform},
    primitive::{
        BitmapFormat, BlendMode, BuiltinFont, CursorHint, EaseCurve, FontFace, HitShape,
        InputRegion, KerningHint, Layer, Primitive, Style,
    },
    AnimatableProperty, InputEnvelope, InputEvent, Node, NodeId, PropertyValue, Rgba, Scene,
    SceneEnvelope, SceneOp,
};

fn nid(n: u32) -> NodeId {
    NodeId::from_parts(0, n)
}

fn sample_node() -> Node {
    Node {
        id: nid(1),
        layer: Layer::Widget,
        transform: Transform::translate(px(10), px(20)),
        primitive: Primitive::Rect {
            w: px(30),
            h: px(40),
            radius_q8: 4,
        },
        style: Style {
            fill: Some(Rgba::RED),
            stroke: Some(Rgba::BLACK),
            stroke_width_q8: 0x100,
            opacity: 200,
            visible: true,
        },
        input: Some(InputRegion {
            shape: HitShape::Aabb {
                w: px(30),
                h: px(40),
            },
            cursor_hint: CursorHint::Pointer,
            capture: true,
        }),
    }
}

fn roundtrip_envelope(op: SceneOp) {
    let env = SceneEnvelope::single(0, op);
    let bytes = encode(&env).expect("encode");
    let back = decode_scene_envelope(&bytes).expect("decode");
    assert_eq!(env, back);
}

#[test]
fn insert_op() {
    roundtrip_envelope(SceneOp::Insert(sample_node()));
}

#[test]
fn update_op() {
    roundtrip_envelope(SceneOp::Update(sample_node()));
}

#[test]
fn remove_op() {
    roundtrip_envelope(SceneOp::Remove(nid(42)));
}

#[test]
fn clear_op() {
    roundtrip_envelope(SceneOp::Clear);
}

#[test]
fn set_layer_blend_op() {
    roundtrip_envelope(SceneOp::SetLayerBlend {
        layer: Layer::Alert,
        mode: BlendMode::Multiply,
    });
}

#[test]
fn tween_op() {
    roundtrip_envelope(SceneOp::Tween {
        id: nid(7),
        property: AnimatableProperty::Opacity,
        from: PropertyValue::Opacity(0),
        to: PropertyValue::Opacity(255),
        duration_ms: 250,
        start_at: Some(50),
        curve: EaseCurve::Cubic {
            c1x: 0x100,
            c1y: 0x200,
            c2x: 0x300,
            c2y: 0x400,
        },
    });
}

#[test]
fn tween_position_op() {
    roundtrip_envelope(SceneOp::Tween {
        id: nid(8),
        property: AnimatableProperty::Position,
        from: PropertyValue::Position(Point::new(0, 0)),
        to: PropertyValue::Position(Point::from_px(100, 50)),
        duration_ms: 500,
        start_at: None,
        curve: EaseCurve::EaseInOut,
    });
}

#[test]
fn cancel_tween_op() {
    roundtrip_envelope(SceneOp::CancelTween {
        id: nid(9),
        property: Some(AnimatableProperty::Fill),
    });
    roundtrip_envelope(SceneOp::CancelTween {
        id: nid(10),
        property: None,
    });
}

#[test]
fn batch_op() {
    roundtrip_envelope(SceneOp::Batch(vec![
        SceneOp::Insert(sample_node()),
        SceneOp::Remove(nid(2)),
        SceneOp::Clear,
    ]));
}

#[test]
fn replace_op() {
    let mut scene = Scene::empty(0);
    scene.bg = Rgba::opaque(10, 20, 30);
    scene.viewport = Rect::from_px(0, 0, 800, 480);
    scene.layer_blend[Layer::Text.index()] = BlendMode::Multiply;
    scene.nodes.push(sample_node());
    roundtrip_envelope(SceneOp::Replace(scene));
}

#[test]
fn text_primitive_carries_full_face_spec() {
    let node = Node {
        id: nid(11),
        layer: Layer::Text,
        transform: Transform::IDENTITY,
        primitive: Primitive::Text {
            content: String::from("13%"),
            face: FontFace::Builtin(BuiltinFont::Mono10x20),
            size_q8: 0x1400, // 20 px
            weight: 600,
            kerning: KerningHint::Auto,
        },
        style: Style::filled(Rgba::WHITE),
        input: None,
    };
    roundtrip_envelope(SceneOp::Insert(node));
}

#[test]
fn bitmap_primitive_roundtrips() {
    let node = Node {
        id: nid(12),
        layer: Layer::Widget,
        transform: Transform::IDENTITY,
        primitive: Primitive::Bitmap {
            w: px(8),
            h: px(8),
            format: BitmapFormat::Raw8888,
            data: vec![0; 8 * 8 * 4],
        },
        style: Style::default(),
        input: None,
    };
    roundtrip_envelope(SceneOp::Insert(node));
}

#[test]
fn input_envelope_pointer_down() {
    let env = InputEnvelope::new(
        0,
        Some(nid(5)),
        InputEvent::PointerDown {
            pointer_id: 1,
            x: px(123),
            y: px(456),
            pressure_q8: 0xFF00,
        },
    );
    let bytes = encode(&env).expect("encode");
    let back = decode_input_envelope(&bytes).expect("decode");
    assert_eq!(env, back);
}

#[test]
fn input_envelope_all_variants() {
    let events = [
        InputEvent::PointerDown {
            pointer_id: 0,
            x: 0,
            y: 0,
            pressure_q8: 0,
        },
        InputEvent::PointerMove {
            pointer_id: 0,
            x: 100,
            y: 100,
            pressure_q8: 0x8000,
        },
        InputEvent::PointerUp {
            pointer_id: 0,
            x: 200,
            y: 200,
        },
        InputEvent::PointerCancel { pointer_id: 0 },
    ];
    for ev in events {
        let env = InputEnvelope::new(0, None, ev);
        let bytes = encode(&env).expect("encode");
        let back = decode_input_envelope(&bytes).expect("decode");
        assert_eq!(env, back);
    }
}

#[test]
fn version_byte_mismatch_rejected() {
    let env = SceneEnvelope {
        version: 7,
        display_id: 0,
        ops: vec![],
    };
    let bytes = encode(&env).expect("encode");
    match decode_scene_envelope(&bytes) {
        Err(CodecError::VersionMismatch { found, expected }) => {
            assert_eq!(found, 7);
            assert_eq!(expected, weftos_leaf_scene::WIRE_VERSION);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn malformed_bytes_decode_error() {
    let result = decode_scene_envelope(&[0xff, 0xff, 0xff, 0xff]);
    assert!(matches!(result, Err(CodecError::Decode)));
}

#[test]
fn cbor_bytes_match_across_calls() {
    // Determinism: same envelope encodes to identical bytes every
    // time. Critical for replay logs and golden tests.
    let env = SceneEnvelope::single(0, SceneOp::Insert(sample_node()));
    let a = encode(&env).expect("encode");
    let b = encode(&env).expect("encode");
    assert_eq!(a, b);
}
