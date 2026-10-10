use std::sync::atomic::{AtomicU64, Ordering};
macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);
        impl $name {
            pub fn next() -> Self {
                static NEXT: AtomicU64 = AtomicU64::new(1);
                Self(NEXT.fetch_add(1, Ordering::Relaxed))
            }
        }
    };
}
identity!(WorkspaceId);
identity!(DocumentId);
identity!(ViewId);
