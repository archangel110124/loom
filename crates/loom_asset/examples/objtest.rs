//! Import a Wavefront OBJ and report what came out.
//!
//! An example rather than a test: the file it reads is a 33 MB download that
//! does not belong in the repository, so a test would either be skipped
//! everywhere or would fail for anyone who has not got it.

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/meshes/trees9.obj".to_owned());
    let path = std::path::Path::new(&path);
    match loom_asset::mesh::import_obj(path) {
        Ok(mesh) => {
            let (lo, hi) = mesh.bounds();
            println!("OK   {} verts, {} indices", mesh.vertices.len(), mesh.indices.len());
            println!(
                "size {:.2} x {:.2} x {:.2} m",
                hi[0] - lo[0],
                hi[1] - lo[1],
                hi[2] - lo[2]
            );
            println!("min  [{:.2} {:.2} {:.2}]", lo[0], lo[1], lo[2]);
            let uvd = mesh.vertices.iter().filter(|v| v.uv != [0.0, 0.0]).count();
            println!("uvs  {uvd} of {} vertices carry one", mesh.vertices.len());
        }
        Err(e) => println!("FAIL {e}"),
    }
}
