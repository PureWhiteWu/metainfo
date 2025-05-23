mod type_map;

use std::{borrow::Cow, collections::HashMap, fmt, sync::Arc};

use fxhash::FxHashMap;
use kv::Node;
use paste::paste;
pub use type_map::TypeMap;

pub mod backward;
pub mod forward;

pub use backward::Backward;
pub use forward::Forward;

mod kv;

#[cfg(feature = "task_local")]
tokio::task_local! {
    pub static METAINFO: std::cell::RefCell<MetaInfo>;
}

/// Framework should all obey these prefixes.
pub const RPC_PREFIX_PERSISTENT: &str = "RPC_PERSIST_";
pub const RPC_PREFIX_TRANSIENT: &str = "RPC_TRANSIT_";
pub const RPC_PREFIX_BACKWARD: &str = "RPC_BACKWARD_";
pub const HTTP_PREFIX_PERSISTENT: &str = "rpc-persist-";
pub const HTTP_PREFIX_TRANSIENT: &str = "rpc-transit-";
pub const HTTP_PREFIX_BACKWARD: &str = "rpc-backward-";

/// `MetaInfo` is used to passthrough information between components and even client-server.
///
/// It supports two types of info: typed map and string k-v.
///
/// It is designed to be tree-like, which means you can share a `MetaInfo` with multiple children.
///
/// Note: only the current scope is mutable.
///
/// Examples:
/// ```rust
/// use metainfo::MetaInfo;
///
/// fn test() {
///     let mut m1 = MetaInfo::new();
///     m1.insert::<i8>(2);
///     assert_eq!(*m1.get::<i8>().unwrap(), 2);
///
///     let (mut m1, mut m2) = m1.derive();
///     assert_eq!(*m2.get::<i8>().unwrap(), 2);
///
///     m2.insert::<i8>(4);
///     assert_eq!(*m2.get::<i8>().unwrap(), 4);
///
///     m2.remove::<i8>();
///     assert_eq!(*m2.get::<i8>().unwrap(), 2);
/// }
/// ```
#[derive(Default)]
pub struct MetaInfo {
    /// Parent is read-only, if we can't find the specified key in the current,
    /// we search it in the parent scope.
    parent: Option<Arc<MetaInfo>>,
    tmap: Option<TypeMap>,
    smap: Option<FxHashMap<Cow<'static, str>, Cow<'static, str>>>, // for str k-v

    /// for information transport through client and server.
    /// e.g. RPC
    forward_node: Option<kv::Node>,
    backward_node: Option<kv::Node>,
}

impl MetaInfo {
    /// Creates an empty `MetaInfo`.
    #[inline]
    pub fn new() -> MetaInfo {
        Default::default()
    }

    /// Creates an `MetaInfo` with the parent given.
    ///
    /// When the info is not found in the current scope, `MetaInfo` will try to get from parent.
    ///
    /// [`derive`] is more efficient than this. It is recommended to use [`derive`] instead of this.
    #[inline]
    pub fn from(parent: Arc<MetaInfo>) -> MetaInfo {
        let forward_node = parent.forward_node.clone();
        let backward_node = parent.backward_node.clone();
        MetaInfo {
            parent: Some(parent),
            tmap: None,
            smap: None,

            forward_node,
            backward_node,
        }
    }

    /// Derives the current [`MetaInfo`], returns two new equivalent `Metainfo`s.
    ///
    /// When the info is not found in the current scope, `MetaInfo` will try to get from parent.
    ///
    /// This is the recommended way.
    #[inline]
    pub fn derive(self) -> (MetaInfo, MetaInfo) {
        if self.tmap.is_none() && self.smap.is_none() {
            // we can use the same parent as self to make the tree small
            let new = MetaInfo {
                parent: self.parent.clone(),
                tmap: None,
                smap: None,
                forward_node: self.forward_node.clone(),
                backward_node: self.backward_node.clone(),
            };
            (self, new)
        } else {
            let mi = Arc::new(self);
            (Self::from(mi.clone()), Self::from(mi))
        }
    }

    /// Insert a type into this `MetaInfo`.
    #[inline]
    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) {
        self.tmap.get_or_insert_with(TypeMap::default).insert(val);
    }

    /// Insert a string k-v into this `MetaInfo`.
    #[inline]
    pub fn insert_string(&mut self, key: Cow<'static, str>, val: Cow<'static, str>) {
        self.smap
            .get_or_insert_with(FxHashMap::default)
            .insert(key, val);
    }

    /// Check if `MetaInfo` contains entry
    #[inline]
    pub fn contains<T: 'static>(&self) -> bool {
        if self
            .tmap
            .as_ref()
            .map(|tmap| tmap.contains::<T>())
            .unwrap_or(false)
        {
            return true;
        }
        self.parent
            .as_ref()
            .map(|parent| parent.as_ref().contains::<T>())
            .unwrap_or(false)
    }

    /// Check if `MetaInfo` contains the given string k-v
    #[inline]
    pub fn contains_string(&self, key: &str) -> bool {
        if self
            .smap
            .as_ref()
            .map(|smap| smap.contains_key(key))
            .unwrap_or(false)
        {
            return true;
        }
        self.parent
            .as_ref()
            .map(|parent| parent.as_ref().contains_string(key))
            .unwrap_or(false)
    }

    /// Get a reference to a type previously inserted on this `MetaInfo`.
    #[inline]
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.tmap.as_ref().and_then(|tmap| tmap.get()).or_else(|| {
            self.parent
                .as_ref()
                .and_then(|parent| parent.as_ref().get::<T>())
        })
    }

    /// Remove a type from this `MetaInfo` and return it.
    /// Can only remove the type in the current scope.
    #[inline]
    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.tmap.as_mut().and_then(|tmap| tmap.remove::<T>())
    }

    /// Get a reference to a string k-v previously inserted on this `MetaInfo`.
    #[inline]
    pub fn get_string(&self, key: &str) -> Option<&Cow<'static, str>> {
        self.smap
            .as_ref()
            .and_then(|smap| smap.get(key))
            .or_else(|| {
                self.parent
                    .as_ref()
                    .and_then(|parent| parent.as_ref().get_string(key))
            })
    }

    /// Remove a string k-v from this `MetaInfo` and return it.
    /// Can only remove the type in the current scope.
    #[inline]
    pub fn remove_string(&mut self, key: &str) -> Option<Cow<'static, str>> {
        self.smap.as_mut().and_then(|smap| smap.remove(key))
    }

    /// Clear the `MetaInfo` of all inserted MetaInfo.
    #[inline]
    pub fn clear(&mut self) {
        if let Some(tmap) = self.tmap.as_mut() {
            tmap.clear()
        }
        if let Some(smap) = self.smap.as_mut() {
            smap.clear()
        }
    }

    /// Extends self with the items from another `MetaInfo`.
    /// Only extend the items in the current scope.
    #[inline]
    pub fn extend(&mut self, other: MetaInfo) {
        if let Some(tmap) = other.tmap {
            self.tmap.get_or_insert_with(TypeMap::default).extend(tmap);
        }

        if let Some(smap) = other.smap {
            self.smap
                .get_or_insert_with(FxHashMap::default)
                .extend(smap);
        }

        if let Some(node) = other.forward_node {
            if self.forward_node.is_none() {
                self.forward_node = Some(node);
            } else {
                self.forward_node.as_mut().unwrap().extend(node);
            }
        }

        if let Some(node) = other.backward_node {
            if self.backward_node.is_none() {
                self.backward_node = Some(node);
            } else {
                self.backward_node.as_mut().unwrap().extend(node);
            }
        }
    }

    fn ensure_forward_node(&mut self) {
        if self.forward_node.is_none() {
            self.forward_node = Some(Node::default())
        }
    }

    fn ensure_backward_node(&mut self) {
        if self.backward_node.is_none() {
            self.backward_node = Some(Node::default())
        }
    }
}

macro_rules! get_impl {
    ($name:ident,$node:ident,$func_name:ident) => {
        paste! {
            fn [<get_ $name>]<K: AsRef<str>>(&self, key: K) -> Option<&str> {
                match self.[<$node _node>].as_ref() {
                    Some(node) => node.[<get_ $func_name>](key),
                    None => None,
                }
            }
        }
    };
}

macro_rules! set_impl {
    ($name:ident,$node:ident,$func_name:ident) => {
        paste! {
            fn [<set_ $name>]<K: Into<Cow<'static, str>>, V: Into<Cow<'static, str>>>(
                &mut self,
                key: K,
                value: V,
            ) {
                self.[<ensure_ $node _node>]();
                self.[<$node _node>]
                    .as_mut()
                    .unwrap()
                    .[<set_ $func_name>](key, value)
            }
        }
    };
}

macro_rules! del_impl {
    ($name:ident,$node:ident,$func_name:ident) => {
        paste! {
            fn [<del_ $name>]<K: AsRef<str>>(&mut self, key: K) {
                if let Some(node) = self.[<$node _node>].as_mut() {
                    node.[<del_ $func_name>](key)
                }
            }
        }
    };
}

impl forward::Forward for MetaInfo {
    get_impl!(persistent, forward, persistent);
    get_impl!(transient, forward, transient);
    get_impl!(upstream, forward, stale);

    set_impl!(persistent, forward, persistent);
    set_impl!(transient, forward, transient);
    set_impl!(upstream, forward, stale);

    del_impl!(persistent, forward, persistent);
    del_impl!(transient, forward, transient);
    del_impl!(upstream, forward, stale);

    fn get_all_persistents(&self) -> Option<&HashMap<Cow<'static, str>, Cow<'static, str>>> {
        match self.forward_node.as_ref() {
            Some(node) => node.get_all_persistents(),
            None => None,
        }
    }

    fn get_all_transients(&self) -> Option<&HashMap<Cow<'static, str>, Cow<'static, str>>> {
        match self.forward_node.as_ref() {
            Some(node) => node.get_all_transients(),
            None => None,
        }
    }

    fn get_all_upstreams(&self) -> Option<&HashMap<Cow<'static, str>, Cow<'static, str>>> {
        match self.forward_node.as_ref() {
            Some(node) => node.get_all_stales(),
            None => None,
        }
    }

    fn strip_rpc_prefix_and_set_persistent<
        K: Into<Cow<'static, str>>,
        V: Into<Cow<'static, str>>,
    >(
        &mut self,
        key: K,
        value: V,
    ) {
        let key: Cow<'static, str> = key.into();
        if let Some(key) = key.strip_prefix(crate::RPC_PREFIX_PERSISTENT) {
            self.set_persistent(key.to_owned(), value);
        }
    }

    fn strip_rpc_prefix_and_set_upstream<K: Into<Cow<'static, str>>, V: Into<Cow<'static, str>>>(
        &mut self,
        key: K,
        value: V,
    ) {
        let key: Cow<'static, str> = key.into();
        if let Some(key) = key.strip_prefix(crate::RPC_PREFIX_TRANSIENT) {
            self.set_upstream(key.to_owned(), value);
        }
    }

    fn strip_http_prefix_and_set_persistent<
        K: Into<Cow<'static, str>>,
        V: Into<Cow<'static, str>>,
    >(
        &mut self,
        key: K,
        value: V,
    ) {
        let key: Cow<'static, str> = key.into();
        if let Some(key) = key.strip_prefix(crate::HTTP_PREFIX_PERSISTENT) {
            self.set_persistent(key.to_owned(), value);
        }
    }

    fn strip_http_prefix_and_set_upstream<
        K: Into<Cow<'static, str>>,
        V: Into<Cow<'static, str>>,
    >(
        &mut self,
        key: K,
        value: V,
    ) {
        let key: Cow<'static, str> = key.into();
        if let Some(key) = key.strip_prefix(crate::HTTP_PREFIX_TRANSIENT) {
            self.set_upstream(key.to_owned(), value);
        }
    }
}

impl backward::Backward for MetaInfo {
    get_impl!(backward_transient, backward, transient);
    get_impl!(backward_downstream, backward, stale);

    set_impl!(backward_transient, backward, transient);
    set_impl!(backward_downstream, backward, stale);

    del_impl!(backward_transient, backward, transient);
    del_impl!(backward_downstream, backward, stale);

    fn get_all_backward_transients(
        &self,
    ) -> Option<&HashMap<Cow<'static, str>, Cow<'static, str>>> {
        match self.backward_node.as_ref() {
            Some(node) => node.get_all_transients(),
            None => None,
        }
    }

    fn get_all_backward_downstreams(
        &self,
    ) -> Option<&HashMap<Cow<'static, str>, Cow<'static, str>>> {
        match self.backward_node.as_ref() {
            Some(node) => node.get_all_stales(),
            None => None,
        }
    }

    fn strip_rpc_prefix_and_set_backward_downstream<
        K: Into<Cow<'static, str>>,
        V: Into<Cow<'static, str>>,
    >(
        &mut self,
        key: K,
        value: V,
    ) {
        let key: Cow<'static, str> = key.into();
        if let Some(key) = key.strip_prefix(crate::RPC_PREFIX_BACKWARD) {
            self.set_backward_downstream(key.to_owned(), value);
        }
    }

    fn strip_http_prefix_and_set_backward_downstream<
        K: Into<Cow<'static, str>>,
        V: Into<Cow<'static, str>>,
    >(
        &mut self,
        key: K,
        value: V,
    ) {
        let key: Cow<'static, str> = key.into();
        if let Some(key) = key.strip_prefix(crate::HTTP_PREFIX_BACKWARD) {
            self.set_backward_downstream(key.to_owned(), value);
        }
    }
}

impl fmt::Debug for MetaInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetaInfo").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove() {
        let mut map = MetaInfo::new();

        map.insert::<i8>(123);
        assert!(map.get::<i8>().is_some());

        map.remove::<i8>();
        assert!(map.get::<i8>().is_none());

        map.insert::<i8>(123);

        let mut m2 = MetaInfo::from(Arc::new(map));

        m2.remove::<i8>();
        assert!(m2.get::<i8>().is_some());
    }

    #[test]
    fn test_clear() {
        let mut map = MetaInfo::new();

        map.insert::<i8>(8);
        map.insert::<i16>(16);
        map.insert::<i32>(32);

        assert!(map.contains::<i8>());
        assert!(map.contains::<i16>());
        assert!(map.contains::<i32>());

        map.clear();

        assert!(!map.contains::<i8>());
        assert!(!map.contains::<i16>());
        assert!(!map.contains::<i32>());

        map.insert::<i8>(10);
        assert_eq!(*map.get::<i8>().unwrap(), 10);
    }

    #[test]
    fn test_integers() {
        let mut map = MetaInfo::new();

        map.insert::<i8>(8);
        map.insert::<i16>(16);
        map.insert::<i32>(32);
        map.insert::<i64>(64);
        map.insert::<i128>(128);
        map.insert::<u8>(8);
        map.insert::<u16>(16);
        map.insert::<u32>(32);
        map.insert::<u64>(64);
        map.insert::<u128>(128);
        assert!(map.get::<i8>().is_some());
        assert!(map.get::<i16>().is_some());
        assert!(map.get::<i32>().is_some());
        assert!(map.get::<i64>().is_some());
        assert!(map.get::<i128>().is_some());
        assert!(map.get::<u8>().is_some());
        assert!(map.get::<u16>().is_some());
        assert!(map.get::<u32>().is_some());
        assert!(map.get::<u64>().is_some());
        assert!(map.get::<u128>().is_some());

        let m2 = MetaInfo::from(Arc::new(map));
        assert!(m2.get::<i8>().is_some());
        assert!(m2.get::<i16>().is_some());
        assert!(m2.get::<i32>().is_some());
        assert!(m2.get::<i64>().is_some());
        assert!(m2.get::<i128>().is_some());
        assert!(m2.get::<u8>().is_some());
        assert!(m2.get::<u16>().is_some());
        assert!(m2.get::<u32>().is_some());
        assert!(m2.get::<u64>().is_some());
        assert!(m2.get::<u128>().is_some());
    }

    #[test]
    fn test_composition() {
        struct Magi<T>(pub T);

        struct Madoka {
            pub god: bool,
        }

        struct Homura {
            pub attempts: usize,
        }

        struct Mami {
            pub guns: usize,
        }

        let mut map = MetaInfo::new();

        map.insert(Magi(Madoka { god: false }));
        map.insert(Magi(Homura { attempts: 0 }));
        map.insert(Magi(Mami { guns: 999 }));

        assert!(!map.get::<Magi<Madoka>>().unwrap().0.god);
        assert_eq!(0, map.get::<Magi<Homura>>().unwrap().0.attempts);
        assert_eq!(999, map.get::<Magi<Mami>>().unwrap().0.guns);
    }

    #[test]
    fn test_metainfo() {
        #[derive(Debug, PartialEq)]
        struct MyType(i32);

        let mut metainfo = MetaInfo::new();

        metainfo.insert(5i32);
        metainfo.insert(MyType(10));

        assert_eq!(metainfo.get(), Some(&5i32));

        assert_eq!(metainfo.remove::<i32>(), Some(5i32));
        assert!(metainfo.get::<i32>().is_none());

        assert_eq!(metainfo.get::<bool>(), None);
        assert_eq!(metainfo.get(), Some(&MyType(10)));
    }

    #[test]
    fn test_extend() {
        #[derive(Debug, PartialEq)]
        struct MyType(i32);

        let mut metainfo = MetaInfo::new();

        metainfo.insert(5i32);
        metainfo.insert(MyType(10));

        let mut other = MetaInfo::new();

        other.insert(15i32);
        other.insert(20u8);

        metainfo.extend(other);

        assert_eq!(metainfo.get(), Some(&15i32));

        assert_eq!(metainfo.remove::<i32>(), Some(15i32));
        assert!(metainfo.get::<i32>().is_none());

        assert_eq!(metainfo.get::<bool>(), None);
        assert_eq!(metainfo.get(), Some(&MyType(10)));

        assert_eq!(metainfo.get(), Some(&20u8));
    }
}
