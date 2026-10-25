//! Hand-written declarations for the part of libmpv's client API the engine uses (`include/mpv/client.h`, client API 2.5, mpv 0.41.0;
//! ADR 0019 choice 13: no binding crate). Layouts are checked by the tests at the bottom.

// Calling C needs `unsafe`; this module and the engine are the only places allowed (crate manifest). Names follow `client.h`.
#![allow(unsafe_code, non_camel_case_types)]

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_ulong, c_void};

use crate::node::Node;

/// `mpv_handle`: opaque.
#[repr(C)]
pub struct mpv_handle {
    _private: [u8; 0],
}

// `mpv_format`
pub const FORMAT_NONE: c_int = 0;
pub const FORMAT_STRING: c_int = 1;
pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;
pub const FORMAT_NODE: c_int = 6;
pub const FORMAT_NODE_ARRAY: c_int = 7;
pub const FORMAT_NODE_MAP: c_int = 8;
pub const FORMAT_BYTE_ARRAY: c_int = 9;

// `mpv_event_id`
pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_LOG_MESSAGE: c_int = 2;
pub const EVENT_SET_PROPERTY_REPLY: c_int = 4;
pub const EVENT_COMMAND_REPLY: c_int = 5;
pub const EVENT_START_FILE: c_int = 6;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_IDLE: c_int = 11;
pub const EVENT_SEEK: c_int = 20;
pub const EVENT_PLAYBACK_RESTART: c_int = 21;
pub const EVENT_PROPERTY_CHANGE: c_int = 22;
pub const EVENT_QUEUE_OVERFLOW: c_int = 24;

#[repr(C)]
#[derive(Clone, Copy)]
pub union mpv_node_u {
    pub string: *mut c_char,
    pub flag: c_int,
    pub int64: i64,
    pub double_: c_double,
    pub list: *mut mpv_node_list,
    pub ba: *mut mpv_byte_array,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct mpv_node {
    pub u: mpv_node_u,
    pub format: c_int,
}

#[repr(C)]
pub struct mpv_node_list {
    pub num: c_int,
    pub values: *mut mpv_node,
    pub keys: *mut *mut c_char,
}

#[repr(C)]
pub struct mpv_byte_array {
    pub data: *mut c_void,
    pub size: usize,
}

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

// The library is named by build.rs (`mpv`), only with the `libmpv` feature.
unsafe extern "C" {
    pub fn mpv_client_api_version() -> c_ulong;
    pub fn mpv_create() -> *mut mpv_handle;
    pub fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    pub fn mpv_terminate_destroy(ctx: *mut mpv_handle);
    pub fn mpv_set_option_string(
        ctx: *mut mpv_handle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    pub fn mpv_set_property_async(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_get_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_command_node_async(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        args: *mut mpv_node,
    ) -> c_int;
    pub fn mpv_observe_property(
        mpv: *mut mpv_handle,
        reply_userdata: u64,
        name: *const c_char,
        format: c_int,
    ) -> c_int;
    pub fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int;
    pub fn mpv_wait_event(ctx: *mut mpv_handle, timeout: c_double) -> *mut mpv_event;
    pub fn mpv_wakeup(ctx: *mut mpv_handle);
    pub fn mpv_free_node_contents(node: *mut mpv_node);
}

/// A C string from mpv, or empty for NULL.
///
/// # Safety
/// `p` is NULL or a NUL-terminated string that stays valid during the call.
pub unsafe fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: non-NULL and NUL-terminated per the contract.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Copies an `mpv_node` tree that mpv owns.
///
/// # Safety
/// `node` is a valid node as `client.h` defines it: its `format` says which union field is set, lists hold `num` values (and keys
/// for maps), and everything stays valid during the call.
pub unsafe fn read_node(node: &mpv_node) -> Node {
    // SAFETY: each arm reads the union field that `format` names, per the contract.
    unsafe {
        match node.format {
            FORMAT_STRING => Node::String(text(node.u.string)),
            FORMAT_FLAG => Node::Flag(node.u.flag != 0),
            FORMAT_INT64 => Node::Int(node.u.int64),
            FORMAT_DOUBLE => Node::Double(node.u.double_),
            FORMAT_NODE_ARRAY | FORMAT_NODE_MAP if !node.u.list.is_null() => {
                let list = &*node.u.list;
                let len = usize::try_from(list.num).unwrap_or(0);
                let values = if len == 0 || list.values.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(list.values, len)
                };
                if node.format == FORMAT_NODE_ARRAY {
                    Node::Array(values.iter().map(|v| read_node(v)).collect())
                } else {
                    let keys = if len == 0 || list.keys.is_null() {
                        &[][..]
                    } else {
                        std::slice::from_raw_parts(list.keys, len)
                    };
                    Node::Map(
                        keys.iter()
                            .zip(values)
                            .map(|(k, v)| (text(*k), read_node(v)))
                            .collect(),
                    )
                }
            }
            FORMAT_BYTE_ARRAY if !node.u.ba.is_null() => {
                let ba = &*node.u.ba;
                if ba.data.is_null() {
                    Node::Bytes(Vec::new())
                } else {
                    Node::Bytes(std::slice::from_raw_parts(ba.data as *const u8, ba.size).to_vec())
                }
            }
            _ => Node::None,
        }
    }
}

/// An `mpv_node` tree built from a [`Node`], owning every allocation it points into, so it stays valid as long as this value lives.
/// mpv copies what it is given (`mpv_command_node_async`: "as in mpv_command_node()"), so it is dropped right after the call.
pub struct CNode {
    root: mpv_node,
    strings: Vec<CString>,
    // Boxed so each list keeps its address while the vector grows: nodes point at them.
    #[allow(clippy::vec_box)]
    lists: Vec<Box<mpv_node_list>>,
    values: Vec<Vec<mpv_node>>,
    keys: Vec<Vec<*mut c_char>>,
}

impl CNode {
    /// Fails if a string or key contains a NUL byte. Byte arrays are not supported (the engine never sends one).
    pub fn new(node: &Node) -> Option<CNode> {
        let mut out = CNode {
            root: mpv_node {
                u: mpv_node_u { int64: 0 },
                format: FORMAT_NONE,
            },
            strings: Vec::new(),
            lists: Vec::new(),
            values: Vec::new(),
            keys: Vec::new(),
        };
        out.root = out.build(node)?;
        Some(out)
    }

    pub fn as_mut_ptr(&mut self) -> *mut mpv_node {
        &mut self.root
    }

    fn string(&mut self, s: &str) -> Option<*mut c_char> {
        let c = CString::new(s).ok()?;
        // The CString's heap buffer does not move when the CString is moved into the vector.
        let p = c.as_ptr() as *mut c_char;
        self.strings.push(c);
        Some(p)
    }

    fn build(&mut self, node: &Node) -> Option<mpv_node> {
        Some(match node {
            Node::None | Node::Bytes(_) => mpv_node {
                u: mpv_node_u { int64: 0 },
                format: FORMAT_NONE,
            },
            Node::String(s) => mpv_node {
                u: mpv_node_u {
                    string: self.string(s)?,
                },
                format: FORMAT_STRING,
            },
            Node::Flag(b) => mpv_node {
                u: mpv_node_u {
                    flag: c_int::from(*b),
                },
                format: FORMAT_FLAG,
            },
            Node::Int(i) => mpv_node {
                u: mpv_node_u { int64: *i },
                format: FORMAT_INT64,
            },
            Node::Double(d) => mpv_node {
                u: mpv_node_u { double_: *d },
                format: FORMAT_DOUBLE,
            },
            Node::Array(items) => {
                let values = items
                    .iter()
                    .map(|n| self.build(n))
                    .collect::<Option<Vec<_>>>()?;
                self.list(values, None, FORMAT_NODE_ARRAY)?
            }
            Node::Map(entries) => {
                let values = entries
                    .iter()
                    .map(|(_, n)| self.build(n))
                    .collect::<Option<Vec<_>>>()?;
                let keys = entries
                    .iter()
                    .map(|(k, _)| self.string(k))
                    .collect::<Option<Vec<_>>>()?;
                self.list(values, Some(keys), FORMAT_NODE_MAP)?
            }
        })
    }

    fn list(
        &mut self,
        mut values: Vec<mpv_node>,
        keys: Option<Vec<*mut c_char>>,
        format: c_int,
    ) -> Option<mpv_node> {
        let num = c_int::try_from(values.len()).ok()?;
        // Moving the vectors into `self` keeps their heap buffers where they are, so these pointers stay valid.
        let values_ptr = values.as_mut_ptr();
        self.values.push(values);
        let keys_ptr = match keys {
            Some(mut keys) => {
                let p = keys.as_mut_ptr();
                self.keys.push(keys);
                p
            }
            None => std::ptr::null_mut(),
        };
        let mut list = Box::new(mpv_node_list {
            num,
            values: values_ptr,
            keys: keys_ptr,
        });
        let list_ptr: *mut mpv_node_list = &mut *list;
        self.lists.push(list);
        Some(mpv_node {
            u: mpv_node_u { list: list_ptr },
            format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    // `client.h` uses no `long` in these structs, so the layout is the same on LP64 (Linux, macOS) and LLP64 (Windows) 64-bit targets:
    // pointers and int64_t 8 bytes, int and enums 4.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn layouts_match_client_h_on_64_bit() {
        assert_eq!(size_of::<mpv_node_u>(), 8);
        assert_eq!(
            (size_of::<mpv_node>(), offset_of!(mpv_node, format)),
            (16, 8)
        );
        assert_eq!(size_of::<mpv_node_list>(), 24);
        assert_eq!(
            (
                offset_of!(mpv_node_list, values),
                offset_of!(mpv_node_list, keys)
            ),
            (8, 16)
        );
        assert_eq!(size_of::<mpv_byte_array>(), 16);
        assert_eq!(size_of::<mpv_event>(), 24);
        assert_eq!(
            (
                offset_of!(mpv_event, error),
                offset_of!(mpv_event, reply_userdata),
                offset_of!(mpv_event, data)
            ),
            (4, 8, 16)
        );
        assert_eq!(size_of::<mpv_event_property>(), 24);
        assert_eq!(
            (
                offset_of!(mpv_event_property, format),
                offset_of!(mpv_event_property, data)
            ),
            (8, 16)
        );
        assert_eq!(size_of::<mpv_event_log_message>(), 32);
        assert_eq!(offset_of!(mpv_event_log_message, log_level), 24);
        assert_eq!(size_of::<mpv_event_end_file>(), 32);
        assert_eq!(
            (
                offset_of!(mpv_event_end_file, error),
                offset_of!(mpv_event_end_file, playlist_entry_id),
                offset_of!(mpv_event_end_file, playlist_insert_id),
                offset_of!(mpv_event_end_file, playlist_insert_num_entries)
            ),
            (4, 8, 16, 24)
        );
    }

    #[test]
    fn built_nodes_read_back_the_same() {
        let node = Node::Map(vec![
            ("name".into(), Node::String("loadfile".into())),
            ("n".into(), Node::Int(-3)),
            ("d".into(), Node::Double(1.25)),
            ("f".into(), Node::Flag(true)),
            (
                "list".into(),
                Node::Array(vec![
                    Node::String("a".into()),
                    Node::Array(vec![]),
                    Node::None,
                ]),
            ),
            (
                "options".into(),
                Node::string_map([("start", "1.000"), ("stream-lavf-o", "max_redirects=%1%0")]),
            ),
        ]);
        let mut c = CNode::new(&node).unwrap();
        // SAFETY: `c` owns a valid tree.
        let back = unsafe { read_node(&*c.as_mut_ptr()) };
        assert_eq!(back, node);
        assert!(CNode::new(&Node::String("a\0b".into())).is_none());
    }
}
