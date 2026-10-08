//! Bake the authored skeleton pose once on the loading worker. Animation
//! playback and additional joint influence sets are deliberately separate work.
use glam::{Mat3, Mat4, Vec3};
use std::borrow::Cow;

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

pub(super) fn palette(
    skin: gltf::Skin<'_>,
    world: &[Option<Mat4>],
    buffers: &[Cow<'_, [u8]>],
) -> Result<Vec<Mat4>, String> {
    let joints: Vec<_> = skin.joints().collect();
    if joints.is_empty() || joints.len() > world.len() {
        return Err("Invalid skin joint count".into());
    }
    let inverse: Vec<_> = if let Some(accessor) = skin.inverse_bind_matrices() {
        if accessor.data_type() != gltf::accessor::DataType::F32
            || accessor.dimensions() != gltf::accessor::Dimensions::Mat4
            || accessor.count() != joints.len()
        {
            return Err("Skin inverse-bind matrices do not match the joints".into());
        }
        crate::model::validate_accessor(&accessor, buffers)?;
        skin.reader(|buffer| buffers.get(buffer.index()).map(|b| b.as_ref()))
            .read_inverse_bind_matrices()
            .ok_or("Skin inverse-bind matrices are unreadable")?
            .map(|matrix| Mat4::from_cols_array_2d(&matrix))
            .collect()
    } else {
        vec![Mat4::IDENTITY; joints.len()]
    };
    joints
        .into_iter()
        .zip(inverse)
        .map(|(joint, inverse)| {
            let world = world[joint.index()].ok_or("Skin joint is outside the selected scene")?;
            let matrix = world * inverse;
            if !matrix.is_finite() {
                return Err("Skin contains a non-finite joint matrix".into());
            }
            Ok(matrix)
        })
        .collect()
}

pub(super) fn bake(
    positions: &mut [[f32; 3]],
    mut normals: Option<&mut [[f32; 3]]>,
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    palette: &[Mat4],
) -> Result<(), String> {
    if joints.len() != positions.len() || weights.len() != positions.len() {
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
        *position = matrix
            .transform_point3(Vec3::from_array(*position))
            .to_array();
        if let Some(normals) = &mut normals {
            let normal_matrix = Mat3::from_mat4(matrix);
            let normal = if normal_matrix.determinant() == 0.0 {
                Vec3::ZERO
            } else {
                (normal_matrix.inverse().transpose() * Vec3::from_array(normals[index]))
                    .normalize_or_zero()
            };
            normals[index] = normal.to_array();
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
        bake(
            &mut positions,
            Some(&mut normals),
            &[[0, 1, 0, 0]],
            &[[0.5, 0.5, 0.0, 0.0]],
            &palette,
        )
        .unwrap();
        assert_eq!(positions[0], [2.0, 3.0, 1.0]);
        assert_eq!(normals[0], [0.0, 0.0, 1.0]);
        for weights in [[0.0; 4], [f32::NAN, 0.0, 0.0, 0.0], [-1.0, 2.0, 0.0, 0.0]] {
            assert!(bake(&mut positions, None, &[[0; 4]], &[weights], &palette).is_err());
        }
        assert!(
            bake(
                &mut positions,
                None,
                &[[99; 4]],
                &[[1.0, 0.0, 0.0, 0.0]],
                &palette
            )
            .is_err()
        );
    }
}
