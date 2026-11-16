//! Loads a data folder's snapshot (no walk, no journal) and prints the file
//! results of some queries: `search_probe <data folder> <query> [query ...]`.

use std::path::PathBuf;
use std::time::Instant;

use prism_core::catalog::FileIndex;

fn main() {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().expect("usage: search_probe <data folder> <query> ..."));
    let index = FileIndex::default();
    let started = Instant::now();
    index.init(&data);
    println!("loaded in {} ms", started.elapsed().as_millis());
    for query in args {
        let started = Instant::now();
        let generation = index.begin_search();
        let response = index.search_with_generation(&query, Some(5), generation);
        println!("\n{query:?}: {} results in {} ms (total {})", response.items.len(), started.elapsed().as_millis(), response.total_indexed);
        for item in &response.items {
            println!("  {}{}", item.path, if item.is_directory { "\\" } else { "" });
        }
    }
}
