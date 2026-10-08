//! `zend_sort` / `zend_insert_sort`, ported from `Zend/zend_sort.c`.
//!
//! Every PHP sort — `sort`, `usort`, `ksort`, `array_multisort`, and the
//! pre-sort of the `array_udiff`/`array_uintersect` family — runs this hybrid
//! insertion sort / quicksort (derived from libc++'s `std::sort`). Which pairs
//! it compares, and in which order, is observable: a user comparator that
//! echoes shows the call sequence, and a comparator that is not a total order
//! (mixed-type `SORT_REGULAR`, a callback returning a bool) yields whatever
//! permutation THIS algorithm produces. A different algorithm, however correct,
//! answers differently, so the C is followed step for step.
//!
//! `cmp` answers like a C comparator: `> 0` means "a sorts after b". Swaps are
//! plain slice swaps, which is what `zend_hash_bucket_swap` amounts to.

/// `zend_sort_2`.
fn sort2<T>(v: &mut [T], a: usize, b: usize, cmp: &mut impl FnMut(&T, &T) -> i32) {
    if cmp(&v[a], &v[b]) > 0 {
        v.swap(a, b);
    }
}

/// `zend_sort_3`.
fn sort3<T>(v: &mut [T], a: usize, b: usize, c: usize, cmp: &mut impl FnMut(&T, &T) -> i32) {
    if !(cmp(&v[a], &v[b]) > 0) {
        if !(cmp(&v[b], &v[c]) > 0) {
            return;
        }
        v.swap(b, c);
        if cmp(&v[a], &v[b]) > 0 {
            v.swap(a, b);
        }
        return;
    }
    if !(cmp(&v[c], &v[b]) > 0) {
        v.swap(a, c);
        return;
    }
    v.swap(a, b);
    if cmp(&v[b], &v[c]) > 0 {
        v.swap(b, c);
    }
}

/// `zend_sort_4`.
fn sort4<T>(
    v: &mut [T],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    cmp: &mut impl FnMut(&T, &T) -> i32,
) {
    sort3(v, a, b, c, cmp);
    if cmp(&v[c], &v[d]) > 0 {
        v.swap(c, d);
        if cmp(&v[b], &v[c]) > 0 {
            v.swap(b, c);
            if cmp(&v[a], &v[b]) > 0 {
                v.swap(a, b);
            }
        }
    }
}

/// `zend_sort_5`.
#[allow(clippy::too_many_arguments)]
fn sort5<T>(
    v: &mut [T],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
    cmp: &mut impl FnMut(&T, &T) -> i32,
) {
    sort4(v, a, b, c, d, cmp);
    if cmp(&v[d], &v[e]) > 0 {
        v.swap(d, e);
        if cmp(&v[c], &v[d]) > 0 {
            v.swap(c, d);
            if cmp(&v[b], &v[c]) > 0 {
                v.swap(b, c);
                if cmp(&v[a], &v[b]) > 0 {
                    v.swap(a, b);
                }
            }
        }
    }
}

/// `zend_insert_sort` over `v[base..base + n]`: networks for up to five
/// elements, then an insertion sort whose search past the sixth element steps
/// back two at a time.
fn insert_sort<T>(v: &mut [T], base: usize, n: usize, cmp: &mut impl FnMut(&T, &T) -> i32) {
    match n {
        0 | 1 => {}
        2 => sort2(v, base, base + 1, cmp),
        3 => sort3(v, base, base + 1, base + 2, cmp),
        4 => sort4(v, base, base + 1, base + 2, base + 3, cmp),
        5 => sort5(v, base, base + 1, base + 2, base + 3, base + 4, cmp),
        _ => {
            let start = base;
            let end = base + n;
            let sentry = start + 6;
            for i in start + 1..sentry {
                let mut j = i - 1;
                if !(cmp(&v[j], &v[i]) > 0) {
                    continue;
                }
                while j != start {
                    j -= 1;
                    if !(cmp(&v[j], &v[i]) > 0) {
                        j += 1;
                        break;
                    }
                }
                rotate_into(v, j, i);
            }
            for i in sentry..end {
                let mut j = i - 1;
                if !(cmp(&v[j], &v[i]) > 0) {
                    continue;
                }
                loop {
                    j -= 2;
                    if !(cmp(&v[j], &v[i]) > 0) {
                        j += 1;
                        if !(cmp(&v[j], &v[i]) > 0) {
                            j += 1;
                        }
                        break;
                    }
                    if j == start {
                        break;
                    }
                    if j == start + 1 {
                        j -= 1;
                        if cmp(&v[i], &v[j]) > 0 {
                            j += 1;
                        }
                        break;
                    }
                }
                rotate_into(v, j, i);
            }
        }
    }
}

/// The C's `for (k = i; k > j; k -= siz) swp(k, k - siz);` — carry `v[i]` down
/// to slot `j` by adjacent swaps.
fn rotate_into<T>(v: &mut [T], j: usize, i: usize) {
    let mut k = i;
    while k > j {
        v.swap(k, k - 1);
        k -= 1;
    }
}

/// `zend_sort`: insertion sort at sixteen elements or fewer, otherwise a
/// quicksort partition around a median-of-three (median-of-five past 1024
/// elements) that recurses into the smaller side and loops on the larger.
pub fn zend_sort<T>(v: &mut [T], cmp: &mut impl FnMut(&T, &T) -> i32) {
    sort_range(v, 0, v.len(), cmp);
}

fn sort_range<T>(v: &mut [T], mut base: usize, mut n: usize, cmp: &mut impl FnMut(&T, &T) -> i32) {
    loop {
        if n <= 16 {
            insert_sort(v, base, n, cmp);
            return;
        }
        let start = base;
        let end = start + n;
        let offset = n >> 1;
        let mut pivot = start + offset;
        if n >> 10 != 0 {
            let delta = offset >> 1;
            sort5(v, start, start + delta, pivot, pivot + delta, end - 1, cmp);
        } else {
            sort3(v, start, pivot, end - 1, cmp);
        }
        v.swap(start + 1, pivot);
        pivot = start + 1;
        let mut i = pivot + 1;
        let mut j = end - 1;
        'partition: loop {
            while cmp(&v[pivot], &v[i]) > 0 {
                i += 1;
                if i == j {
                    break 'partition;
                }
            }
            j -= 1;
            if j == i {
                break 'partition;
            }
            while cmp(&v[j], &v[pivot]) > 0 {
                j -= 1;
                if j == i {
                    break 'partition;
                }
            }
            v.swap(i, j);
            i += 1;
            if i == j {
                break 'partition;
            }
        }
        v.swap(pivot, i - 1);
        if (i - 1) - start < end - i {
            sort_range(v, start, (i - start) - 1, cmp);
            base = i;
            n = end - i;
        } else {
            sort_range(v, i, end - i, cmp);
            n = (i - start) - 1;
        }
    }
}

/// `zend_hash_sort` with a `RETURN_STABLE_SORT` comparator: ties the
/// comparator reports as `0` are broken by original position (`Z_EXTRA`), which
/// is how PHP 8 makes every sort stable while still running `zend_sort`.
pub fn zend_sort_stable<T>(v: &mut Vec<T>, mut cmp: impl FnMut(&T, &T) -> i32) {
    if v.len() < 2 {
        return;
    }
    let mut tagged: Vec<(usize, T)> = std::mem::take(v).into_iter().enumerate().collect();
    zend_sort(&mut tagged, &mut |(ia, a), (ib, b)| match cmp(a, b) {
        0 => (*ia).cmp(ib) as i32,
        r => r,
    });
    *v = tagged.into_iter().map(|(_, t)| t).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comparison sequence for a 21-element `usort`, as the reference prints
    /// it with an echoing comparator — the first 25 pairs pin the partition
    /// step and the recursion into the smaller side.
    #[test]
    fn compare_sequence_matches_reference() {
        let mut a = vec![
            5, 3, 8, 1, 9, 2, 7, 4, 6, 0, 15, 12, 11, 14, 13, 10, 20, 18, 19, 17, 16,
        ];
        let mut seen = Vec::new();
        zend_sort_stable(&mut a, |x, y| {
            seen.push((*x, *y));
            (*x).cmp(y) as i32
        });
        let want = [
            (5, 15),
            (15, 16),
            (15, 8),
            (15, 1),
            (15, 9),
            (15, 2),
            (15, 7),
            (15, 4),
            (15, 6),
            (15, 0),
            (15, 3),
            (15, 12),
            (15, 11),
            (15, 14),
            (15, 13),
            (15, 10),
            (15, 20),
            (17, 15),
            (19, 15),
            (18, 15),
            (20, 18),
            (19, 18),
            (20, 19),
            (20, 17),
            (19, 17),
        ];
        assert_eq!(&seen[..want.len()], &want);
        assert_eq!(a, (0..=20).collect::<Vec<_>>());
    }

    #[test]
    fn inconsistent_comparator_does_not_panic() {
        for n in 0..200 {
            let mut v: Vec<u32> = (0..n).collect();
            let mut s = 7u32;
            zend_sort(&mut v, &mut |_, _| {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                (s >> 16) as i32 % 3 - 1
            });
            assert_eq!(v.len(), n as usize);
        }
    }
}
