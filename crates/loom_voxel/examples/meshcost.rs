//! What the mesh actually costs today.
fn main() {
    use loom_voxel::*;
    let mut v = Volume::new([4,3,4], 0.25);
    v.bake(&[
        VoxelOp::Box{center:[16.0,3.0,16.0],half_extents:[12.0,3.0,12.0],mode:CsgMode::Union, displace: None},
        VoxelOp::Sphere{center:[16.0,7.0,16.0],radius:8.5,mode:CsgMode::Union, displace: None},
        VoxelOp::Capsule{a:[5.0,7.0,16.0],b:[27.0,7.0,16.0],radius:2.0,mode:CsgMode::Subtract, displace: None},
    ]);
    let m = mesh::mesh_volume(&v, &SurfaceNets);
    let verts = m.vertices.len();
    let tris = m.indices.len()/3;
    let raw = verts * std::mem::size_of::<loom_asset::Vertex>();
    let (packed, _) = loom_asset::packed::pack(&m.vertices);
    let pk = packed.len() * std::mem::size_of::<loom_asset::PackedVertex>();
    let ibytes = m.indices.len() * 4;
    println!("vertices           {verts}");
    println!("triangles          {tris}");
    println!("vertex, uncompressed  {:>2} bytes  {:>5} KB", std::mem::size_of::<loom_asset::Vertex>(), raw/1024);
    println!("vertex, packed        {:>2} bytes  {:>5} KB   ({:.0}% saved)",
             std::mem::size_of::<loom_asset::PackedVertex>(), pk/1024,
             (1.0 - pk as f64/raw as f64)*100.0);
    println!("indices               {:>2} bytes  {:>5} KB", 4, ibytes/1024);
    println!("TOTAL uploaded              {:>5} KB  (was {} KB)", (pk+ibytes)/1024, (raw+ibytes)/1024);
    println!();
    println!("per-chunk vertex counts (max determines u16 index viability):");
    let mut worst = 0;
    for c in v.surface_chunks() { worst = worst.max(SurfaceNets.mesh_chunk(&v, c).positions.len()); }
    println!("  worst chunk       {worst} verts  -> u16 indices {}", if worst < 65536 {"VIABLE"} else {"no"});
    println!("  surface chunks    {}", v.surface_chunks().len());
}
