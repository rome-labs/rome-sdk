pub fn into_chunks<T>(mut vec: Vec<T>, chunk_size: usize) -> Vec<Vec<T>> {
    if chunk_size == 0 {
        panic!("chunk_size must be greater than 0");
    }

    let mut chunks = Vec::new();

    while !vec.is_empty() {
        let chunk: Vec<T> = vec.drain(..chunk_size.min(vec.len())).collect();
        chunks.push(chunk);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_chunks_exact_division() {
        let vec = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        let chunk_size = 3;
        let result = into_chunks(vec, chunk_size);
        assert_eq!(result, vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]]);
    }

    #[test]
    fn test_into_chunks_with_remainder() {
        let vec = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let chunk_size = 3;
        let result = into_chunks(vec, chunk_size);
        assert_eq!(
            result,
            vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9], vec![10]]
        );
    }

    #[test]
    fn test_into_chunks_single_chunk() {
        let vec = vec![1, 2, 3, 4, 5];
        let chunk_size = 10;
        let result = into_chunks(vec, chunk_size);
        assert_eq!(result, vec![vec![1, 2, 3, 4, 5]]);
    }

    #[test]
    fn test_into_chunks_single_element_chunks() {
        let vec = vec![1, 2, 3, 4, 5];
        let chunk_size = 1;
        let result = into_chunks(vec, chunk_size);
        assert_eq!(result, vec![vec![1], vec![2], vec![3], vec![4], vec![5]]);
    }

    #[test]
    fn test_into_chunks_empty_vec() {
        let vec: Vec<i32> = Vec::new();
        let chunk_size = 3;
        let result = into_chunks(vec, chunk_size);
        assert_eq!(result, Vec::<Vec<i32>>::new());
    }

    #[test]
    #[should_panic(expected = "chunk_size must be greater than 0")]
    fn test_into_chunks_chunk_size_zero() {
        let vec = vec![1, 2, 3, 4, 5];
        let chunk_size = 0;
        into_chunks(vec, chunk_size); // This should panic
    }
}
