//! Workspace logos are optional 64 × 64, straight-alpha sRGB RGBA thumbnails.
//! An empty buffer means no logo. Encoded files and local paths never cross this boundary.
pub const SIDE: u32 = 64;
pub const BYTE_LEN: usize = SIDE as usize * SIDE as usize * 4;

pub fn valid(bytes: &[u8]) -> bool {
    bytes.is_empty() || bytes.len() == BYTE_LEN
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::UpdateWorkspaceFoundationRequest;
    use prost::Message;

    #[test]
    fn accepts_only_absence_or_a_complete_thumbnail() {
        assert!(valid(&[]));
        assert!(valid(&vec![0; BYTE_LEN]));
        assert!(!valid(&vec![0; BYTE_LEN - 1]));
        assert!(!valid(&vec![0; BYTE_LEN + 1]));
    }

    #[test]
    fn wire_distinguishes_preserve_remove_and_replace() {
        for logo_rgba in [None, Some(Vec::new()), Some(vec![100; BYTE_LEN])] {
            let request = UpdateWorkspaceFoundationRequest {
                logo_rgba,
                ..Default::default()
            };
            let decoded =
                UpdateWorkspaceFoundationRequest::decode(request.encode_to_vec().as_slice())
                    .unwrap();
            assert_eq!(decoded.logo_rgba, request.logo_rgba);
        }
    }
}
