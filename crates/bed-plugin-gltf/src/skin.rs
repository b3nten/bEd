//! Scene traversal and authored-pose inspection of Bevy-owned meshes.
use glam::{Mat3, Mat4, Vec3};

pub(super) fn scene_nodes<'a>(
    scene: gltf::Scene<'a>,
    count: usize,
) -> Result<Vec<(gltf::Node<'a>, Mat4)>, String> {
    let mut stack: Vec<_> = scene.nodes().map(|node| (node, Mat4::IDENTITY)).collect();
    let mut seen = vec![false; count];
    let mut nodes = Vec::new();
    while let Some((node, parent)) = stack.pop() {
        if std::mem::replace(&mut seen[node.index()], true) {
            return Err("glTF scene contains a cycle or a node with multiple parents".into());
        }
        let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if !transform.is_finite() {
            return Err("glTF node has a non-finite transform".into());
        }
        stack.extend(node.children().map(|child| (child, transform)));
        nodes.push((node, transform));
    }
    Ok(nodes)
}

pub(super) fn bake(
    positions: &mut [[f32; 3]],
    mut normals: Option<&mut [[f32; 3]]>,
    mut tangents: Option<&mut [[f32; 4]]>,
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    palette: &[Mat4],
) -> Result<(), String> {
    if joints.len() != positions.len()
        || weights.len() != positions.len()
        || normals
            .as_ref()
            .is_some_and(|values| values.len() != positions.len())
        || tangents
            .as_ref()
            .is_some_and(|values| values.len() != positions.len())
    {
        return Err("Skin joint/weight attributes have mismatched lengths".into());
    }
    for (index, position) in positions.iter_mut().enumerate() {
        let weights = weights[index];
        let total: f32 = weights.iter().sum();
        if !total.is_finite() || total <= 0.0 || weights.iter().any(|w| !w.is_finite() || *w < 0.0)
        {
            return Err("Skin weights must be finite, nonnegative and have a positive sum".into());
        }
        let mut matrix = Mat4::ZERO;
        for (joint, weight) in joints[index].into_iter().zip(weights) {
            let joint = palette
                .get(usize::from(joint))
                .ok_or("Skin vertex references a missing joint")?;
            matrix += *joint * (weight / total);
        }
        let determinant = Mat3::from_mat4(matrix).determinant();
        if !matrix.is_finite() || !determinant.is_finite() {
            return Err("Skin blend exceeds the preview's numeric range".into());
        }
        *position = matrix
            .transform_point3(Vec3::from_array(*position))
            .to_array();
        if let Some(normals) = &mut normals {
            let normal_matrix = Mat3::from_mat4(matrix);
            let normal = if determinant == 0.0 {
                Vec3::ZERO
            } else {
                (normal_matrix.inverse().transpose() * Vec3::from_array(normals[index]))
                    .normalize_or_zero()
            };
            normals[index] = normal.to_array();
        }
        if let Some(tangents) = &mut tangents {
            let tangent = tangents[index];
            let direction = matrix
                .transform_vector3(Vec3::new(tangent[0], tangent[1], tangent[2]))
                .normalize_or_zero();
            tangents[index] = [
                direction.x,
                direction.y,
                direction.z,
                tangent[3] * determinant.signum(),
            ];
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_skin_blends_joint_transforms_and_rejects_bad_influences() {
        let palette = [
            Mat4::from_translation(Vec3::X * 2.0),
            Mat4::from_translation(Vec3::Y * 4.0),
        ];
        let mut positions = [[1.0, 1.0, 1.0]];
        let mut normals = [[0.0, 0.0, 1.0]];
        let mut tangents = [[1.0, 0.0, 0.0, 1.0]];
        bake(
            &mut positions,
            Some(&mut normals),
            Some(&mut tangents),
            &[[0, 1, 0, 0]],
            &[[0.5, 0.5, 0.0, 0.0]],
            &palette,
        )
        .unwrap();
        assert_eq!(positions[0], [2.0, 3.0, 1.0]);
        assert_eq!(normals[0], [0.0, 0.0, 1.0]);
        assert_eq!(tangents[0], [1.0, 0.0, 0.0, 1.0]);
        bake(
            &mut positions,
            None,
            Some(&mut tangents),
            &[[0; 4]],
            &[[1.0, 0.0, 0.0, 0.0]],
            &[Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))],
        )
        .unwrap();
        assert_eq!(tangents[0], [-1.0, 0.0, 0.0, -1.0]);
        for weights in [[0.0; 4], [f32::NAN, 0.0, 0.0, 0.0], [-1.0, 2.0, 0.0, 0.0]] {
            assert!(bake(&mut positions, None, None, &[[0; 4]], &[weights], &palette).is_err());
        }
        assert!(
            bake(
                &mut positions,
                None,
                None,
                &[[99; 4]],
                &[[1.0, 0.0, 0.0, 0.0]],
                &palette
            )
            .is_err()
        );
    }

    #[test]
    fn static_skin_transforms_tangent_basis_with_nonuniform_scale() {
        let mut positions = [[1.0, 2.0, 3.0]];
        let normal = Vec3::new(1.0, 1.0, 0.0).normalize();
        let tangent = Vec3::new(1.0, -1.0, 0.0).normalize();
        let mut normals = [normal.to_array()];
        let mut tangents = [[tangent.x, tangent.y, tangent.z, 1.0]];
        bake(
            &mut positions,
            Some(&mut normals),
            Some(&mut tangents),
            &[[0; 4]],
            &[[1.0, 0.0, 0.0, 0.0]],
            &[Mat4::from_scale(Vec3::new(-2.0, 3.0, 4.0))],
        )
        .unwrap();
        assert_eq!(positions[0], [-2.0, 6.0, 12.0]);
        let normal = Vec3::from_array(normals[0]);
        let tangent = Vec3::from_slice(&tangents[0][..3]);
        assert!((normal.length() - 1.0).abs() < 1e-6);
        assert!((tangent.length() - 1.0).abs() < 1e-6);
        assert!(normal.dot(tangent).abs() < 1e-6);
        assert_eq!(tangents[0][3], -1.0);
    }

    #[test]
    fn static_skin_rejects_mismatched_tangent_attributes() {
        assert!(
            bake(
                &mut [[0.0; 3]],
                None,
                Some(&mut []),
                &[[0; 4]],
                &[[1.0, 0.0, 0.0, 0.0]],
                &[Mat4::IDENTITY],
            )
            .is_err()
        );
    }
}
