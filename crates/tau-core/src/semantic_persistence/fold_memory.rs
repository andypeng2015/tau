//! Conservative requested-allocation bounds for synchronous producer folds.
//!
//! These bounds cover new ownership and transient overlap, not the existing
//! authoritative projection or allocator RSS. Saturation always rejects
//! admission.

use std::fmt;

use serde::Serialize;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};

/// Scratch for the encoded-size pass, before reserving validation and fold
/// ownership. Ordinary strings stream directly into the counting writer;
/// opaque context serializers additionally parse their raw JSON.
pub(crate) fn serialization_scratch(event: &tau_proto::Event) -> usize {
    use tau_proto::Event;
    let (items, formatting): (&[tau_proto::ContextItem], usize) = match event {
        Event::ProviderResponseFinished(response) => (
            &response.output_items,
            formatting_scratch(&(
                &response.context_limit_telemetry,
                &response.automatic_compaction_decision,
                &response.usage,
            )),
        ),
        Event::ProviderResponseFinishedReported(response) => (
            &response.output_items,
            formatting_scratch(&(
                &response.context_limit_telemetry,
                &response.automatic_compaction_decision,
                &response.usage,
            )),
        ),
        Event::AgentCompacted(compacted) => (
            &compacted.replacement_window,
            formatting_scratch(&compacted.model),
        ),
        Event::AgentStandaloneCompactionFailed(failed) => (
            failed
                .incomplete_response
                .as_ref()
                .map_or(&[], |response| &response.output_items),
            formatting_scratch(&(
                &failed.context_retreat,
                failed
                    .incomplete_response
                    .as_ref()
                    .and_then(|response| response.usage.as_ref()),
            )),
        ),
        Event::AgentPromptCreated(_) | Event::AgentPromptPrewarmRequested(_) => return usize::MAX,
        _ => (&[], formatting_scratch(event)),
    };
    items.iter().fold(
        (64 * 1024usize).saturating_add(formatting),
        |charge, item| {
            charge.saturating_add(match item {
                tau_proto::ContextItem::Reasoning(opaque)
                | tau_proto::ContextItem::Compaction(opaque)
                | tau_proto::ContextItem::UnknownProviderItem(opaque) => {
                    json_scratch(opaque.raw_json())
                }
                _ => 0,
            })
        },
    )
}

/// Ciborium inherits Serde's allocating `collect_str` default. Count every
/// formatted field without allocating its string, including nested ModelIds.
/// Context-item carriers are split by the caller to avoid parsing opaque JSON
/// before its reservation. Their remaining fields and persisted record metadata
/// contain no unbounded formatting serializers.
fn formatting_scratch(value: &(impl Serialize + ?Sized)) -> usize {
    let mut charge = ValueCharge::default();
    if charge.value(value).is_err() {
        usize::MAX
    } else {
        // Geometric String growth and old/new backing overlap.
        charge.formatted_bytes.saturating_mul(4)
    }
}

/// Reserves validation scratch before invoking custom `ContextItem::serialize`,
/// which reparses opaque raw JSON even during a serialized-size counting pass.
/// The raw lexical bound also covers overwritten duplicate object keys rather
/// than relying only on the retained, deduplicated structured representation.
pub(crate) fn validation_scratch(event: &tau_proto::Event) -> usize {
    use tau_proto::Event;
    let items: &[tau_proto::ContextItem] = match event {
        Event::ProviderResponseFinished(response) => &response.output_items,
        Event::ProviderResponseFinishedReported(response) => &response.output_items,
        Event::AgentCompacted(compacted) => &compacted.replacement_window,
        Event::AgentStandaloneCompactionFailed(failed) => failed
            .incomplete_response
            .as_ref()
            .map_or(&[], |response| &response.output_items),
        Event::ProviderToolResult(result) | Event::ToolResult(result) => {
            return owned_value(event)
                .saturating_mul(4)
                .saturating_add(64 * 1024)
                .saturating_add(inline_image_output(&result.provider_content));
        }
        // Full materialized prompts are not semantic journal records. Do not
        // invoke their nested custom serializers while sizing rejected input.
        Event::AgentPromptCreated(_) | Event::AgentPromptPrewarmRequested(_) => return usize::MAX,
        _ => {
            return owned_value(event)
                .saturating_mul(4)
                .saturating_add(64 * 1024);
        }
    };
    let image_output = items
        .iter()
        .map(|item| match item {
            tau_proto::ContextItem::ToolResult(result) => {
                inline_image_output(&result.provider_content)
            }
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    items.iter().fold(
        (64 * 1024usize).saturating_add(image_output),
        |charge, item| {
            charge
                .saturating_add(context_item(item).saturating_mul(4))
                .saturating_add(match item {
                    tau_proto::ContextItem::Reasoning(opaque)
                    | tau_proto::ContextItem::Compaction(opaque)
                    | tau_proto::ContextItem::UnknownProviderItem(opaque) => {
                        json_scratch(opaque.raw_json())
                    }
                    _ => 0,
                })
        },
    )
}

/// Existing validation bounds the final decoded image buffer separately from
/// third-party codec internals. Those pre-existing codec-internal allocations
/// are not covered by this persistence-owned requested-capacity ledger.
fn inline_image_output(parts: &[tau_proto::ToolResultContentPart]) -> usize {
    parts
        .iter()
        .map(|part| match part {
            tau_proto::ToolResultContentPart::Image(image) => {
                if image.media_type == tau_proto::ImageMediaType::Webp {
                    32 * 1024 * 1024
                } else {
                    64 * 1024 * 1024
                }
            }
            tau_proto::ToolResultContentPart::ImageReference(_) => 0,
        })
        .max()
        .unwrap_or(0)
}

/// Counts one context payload without invoking its opaque-validation
/// serializer.
pub(crate) fn context_item(item: &tau_proto::ContextItem) -> usize {
    use tau_proto::ContextItem;
    let payload = match item {
        ContextItem::Message(value) => owned_value(value),
        ContextItem::ToolCall(value) => owned_value(value),
        ContextItem::ToolResult(value) => owned_value(value),
        ContextItem::ReasoningText(value) => owned_value(value),
        ContextItem::LocalCompactionNarrative(value) => owned_value(value),
        ContextItem::Reasoning(value)
        | ContextItem::Compaction(value)
        | ContextItem::UnknownProviderItem(value) => {
            owned_value(value.value()).saturating_add(value.raw_json().len())
        }
        ContextItem::CompactionTrigger => 0,
    };
    payload.saturating_add(size_of::<tau_proto::ContextItem>())
}

/// Bounds JSON parser strings, value slots, map nodes, geometric container
/// growth, and parsing scratch without interpreting or accepting the JSON.
fn json_scratch(raw: &str) -> usize {
    let mut quoted = false;
    let mut escaped = false;
    let mut structure = 0usize;
    for byte in raw.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
            structure = structure.saturating_add(1);
        } else if !byte.is_ascii_whitespace() {
            structure = structure.saturating_add(1);
        }
    }
    raw.len()
        .saturating_mul(4)
        .saturating_add(structure.saturating_mul(512))
}

/// Bounds new Vec backing allocations when up to `additional` elements are
/// added. The geometric-growth allowance includes overlap and small initial
/// allocations.
pub(crate) fn vec_growth<T>(values: &Vec<T>, additional: usize) -> usize {
    let needed = values.len().saturating_add(additional);
    if needed <= values.capacity() {
        return 0;
    }
    needed
        .max(8)
        .saturating_mul(4)
        .saturating_mul(size_of::<T>())
}

/// Bounds hash-table backing growth, including buckets, control bytes,
/// alignment, and geometric overlap. This deliberately overestimates the
/// std/hashbrown load-factor and growth policy; it is not an allocator
/// usable-size estimate.
pub(crate) fn hash_growth<T>(len: usize, capacity: usize, additional: usize) -> usize {
    let needed = len.saturating_add(additional);
    if needed <= capacity {
        return 0;
    }
    needed
        .saturating_add(1)
        .saturating_mul(16)
        .saturating_mul(size_of::<T>().saturating_add(1))
        .saturating_add(64)
}

/// Bounds owned clones of serialized semantic values without encoding or
/// allocating a second value. Typed inline element sizes account for wide enums
/// even when their wire encoding is tiny; strings and bytes are counted by
/// length. Per-entry padding covers container nodes/buckets and geometric
/// growth. Skipped empty fields allocate nothing when cloned. Non-serialized
/// fold state must be charged separately by its owning projection.
pub(crate) fn owned_value(value: &(impl Serialize + ?Sized)) -> usize {
    let mut charge = ValueCharge::default();
    if charge.value(value).is_err() {
        usize::MAX
    } else {
        charge.bytes
    }
}

/// Nonretaining structural walk of an owned semantic value.
#[derive(Default)]
struct ValueCharge {
    /// Checked requested bytes accumulated by the walk.
    bytes: usize,
    /// Bytes produced by Display-based serializers without retaining a String.
    formatted_bytes: usize,
}

impl ValueCharge {
    /// Includes inline storage and container overhead before visiting children.
    fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ChargeOverflow> {
        self.add(
            std::mem::size_of_val(value)
                .checked_mul(4)
                .ok_or(ChargeOverflow)?,
        )?;
        self.add(64)?;
        value.serialize(self)
    }

    /// Adds a term without permitting an overflow to wrap into an admissible
    /// size.
    fn add(&mut self, bytes: usize) -> Result<(), ChargeOverflow> {
        self.bytes = self.bytes.checked_add(bytes).ok_or(ChargeOverflow)?;
        Ok(())
    }
}

/// Overflow or an unsupported custom serialization fails closed before
/// admission.
#[derive(Debug)]
struct ChargeOverflow;

impl fmt::Display for ChargeOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fold allocation charge overflow")
    }
}

impl std::error::Error for ChargeOverflow {}

impl serde::ser::Error for ChargeOverflow {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self
    }
}

macro_rules! scalar {
    ($($method:ident($ty:ty)),* $(,)?) => {$(
        fn $method(self, _value: $ty) -> Result<(), ChargeOverflow> { Ok(()) }
    )*};
}

impl serde::Serializer for &mut ValueCharge {
    type Ok = ();
    type Error = ChargeOverflow;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    scalar! {
        serialize_bool(bool), serialize_i8(i8), serialize_i16(i16), serialize_i32(i32),
        serialize_i64(i64), serialize_i128(i128), serialize_u8(u8), serialize_u16(u16),
        serialize_u32(u32), serialize_u64(u64), serialize_u128(u128), serialize_f32(f32),
        serialize_f64(f64), serialize_char(char)
    }

    fn serialize_str(self, value: &str) -> Result<(), ChargeOverflow> {
        self.add(value.len())
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), ChargeOverflow> {
        self.add(value.len())
    }
    fn serialize_none(self) -> Result<(), ChargeOverflow> {
        Ok(())
    }
    fn serialize_unit(self) -> Result<(), ChargeOverflow> {
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), ChargeOverflow> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<(), ChargeOverflow> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), ChargeOverflow> {
        self.value(value)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), ChargeOverflow> {
        self.value(value)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<(), ChargeOverflow> {
        self.value(value)
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self, ChargeOverflow> {
        self.add(64)?;
        Ok(self)
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self, ChargeOverflow> {
        Ok(self)
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self, ChargeOverflow> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self, ChargeOverflow> {
        Ok(self)
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self, ChargeOverflow> {
        self.add(64)?;
        Ok(self)
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self, ChargeOverflow> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self, ChargeOverflow> {
        Ok(self)
    }
    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<(), ChargeOverflow> {
        struct Counter<'a>(&'a mut ValueCharge);
        impl fmt::Write for Counter<'_> {
            fn write_str(&mut self, value: &str) -> fmt::Result {
                self.0.formatted_bytes = self
                    .0
                    .formatted_bytes
                    .checked_add(value.len())
                    .ok_or(fmt::Error)?;
                self.0.add(value.len()).map_err(|_| fmt::Error)
            }
        }
        fmt::write(&mut Counter(self), format_args!("{value}")).map_err(|_| ChargeOverflow)
    }
    fn is_human_readable(&self) -> bool {
        false
    }
}

macro_rules! sequence {
    ($trait:ident, $method:ident) => {
        impl $trait for &mut ValueCharge {
            type Ok = ();
            type Error = ChargeOverflow;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ChargeOverflow> {
                self.value(value)
            }
            fn end(self) -> Result<(), ChargeOverflow> {
                Ok(())
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);

impl SerializeMap for &mut ValueCharge {
    type Ok = ();
    type Error = ChargeOverflow;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ChargeOverflow> {
        self.value(value)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ChargeOverflow> {
        self.value(value)
    }
    fn end(self) -> Result<(), ChargeOverflow> {
        Ok(())
    }
}

macro_rules! structure {
    ($trait:ident) => {
        impl $trait for &mut ValueCharge {
            type Ok = ();
            type Error = ChargeOverflow;
            fn serialize_field<T: Serialize + ?Sized>(
                &mut self,
                _key: &'static str,
                value: &T,
            ) -> Result<(), ChargeOverflow> {
                self.value(value)
            }
            fn end(self) -> Result<(), ChargeOverflow> {
                Ok(())
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);
