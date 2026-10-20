pub fn compose_vector<T, U: FnMut(usize, &mut [T]) -> T>(
    n: usize,
    mut f: U,
) -> Vec<T> {
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let val = f(i, &mut v);
        v.push(val);
    }
    v
}
