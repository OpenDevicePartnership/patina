use patina::component::{Query, Storage};

struct Value;

fn main() {
    let mut storage = Storage::new();
    let mut mutable_query = Query::<&mut Value>::new().query_mut(&mut storage);
    let _shared_query = Query::<&Value>::new().query(&storage);
    let _ = mutable_query.next();
}
