//! Exact-rational lattice reduction (LLL) and lattice-point enumeration.
//!
//! This module is the numeric engine behind the lattice-based cracking modes:
//! given a lattice and an axis-aligned box, it produces *every* lattice point
//! inside that box. It is a port of the maths in mjtb49's LattiCG (itself
//! following Henri Cohen's *A Course in Computational Algebraic Number
//! Theory*, algorithm 2.6.3), with the enumeration step swapped for classical
//! Fincke-Pohst — see [`enumerate_in_box`] for why.
//!
//! # Why exact rationals
//!
//! Floating-point LLL is faster and is what most libraries ship, but it fails
//! in a way that is invisible: a slightly wrong Gram-Schmidt coefficient makes
//! the reduction merely *worse*, while a slightly wrong enumeration bound makes
//! the search *incomplete*. A missed lattice point here is a seed the tool
//! silently claims does not exist. Every value below is a [`num_rational`]
//! `BigRational`, so there is no drift and no epsilon to tune — only speed to
//! pay for it.
//!
//! # Conventions
//!
//! * Lattices are given by their basis **rows**. `RatMatrix` is row-major and
//!   every operation here (Gram-Schmidt, size reduction, swaps) works on rows.
//! * The Gram-Schmidt coefficient matrix `mu` is lower-triangular with
//!   `mu[k][k] == 1`, so `b_k == sum_{j<=k} mu[k][j] * b*_j`.
//! * Rounding in the size-reduction step is **half away from zero** (that is
//!   `num_rational`'s `round`). LattiCG rounds half *up* (`floor(x + 1/2)`).
//!   The two differ only on exact halves and only pick a different — equally
//!   valid — reduced basis, so no attempt is made to match LattiCG bit for bit.
//!
//! Nothing in here depends on the rest of the crate, and it must stay that way:
//! it is pure maths, and pure maths is the only part of a seed cracker that can
//! be tested to the last bit.

use anyhow::{Result, bail, ensure};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

/// Exact rational scalar. Every number in this module is one of these.
pub type Rat = BigRational;

/// `n` as a rational. Convenience for building small matrices and bounds.
pub fn rat(n: i64) -> Rat {
    Rat::from_integer(BigInt::from(n))
}

/// `n / d` as a rational. Panics if `d == 0`.
pub fn ratio(n: i64, d: i64) -> Rat {
    Rat::new(BigInt::from(n), BigInt::from(d))
}

/// Euclidean inner product. Panics on a length mismatch — every caller in this
/// module has already checked dimensions.
fn dot(a: &[Rat], b: &[Rat]) -> Rat {
    debug_assert_eq!(a.len(), b.len());
    let mut acc = Rat::zero();
    for (x, y) in a.iter().zip(b.iter()) {
        acc += x * y;
    }
    acc
}

/// `a -= s * b`, elementwise.
fn sub_scaled(a: &mut [Rat], b: &[Rat], s: &Rat) {
    for (x, y) in a.iter_mut().zip(b.iter()) {
        *x -= s * y;
    }
}

/// Smallest integer whose square is at least `n`. `n` must be non-negative.
fn ceil_sqrt(n: &BigInt) -> BigInt {
    let s = n.sqrt(); // floor
    if &s * &s == *n { s } else { s + 1 }
}

/// A rational that is guaranteed to be **at least** `sqrt(q)`, for `q >= 0`.
///
/// Fincke-Pohst needs a square root, which is irrational in general, so we
/// never take one: we bound it. Writing `q = p/r` with `r > 0`,
/// `sqrt(p/r) <= ceil(sqrt(p)) / floor(sqrt(r))` because the numerator only
/// grows and the (positive) denominator only shrinks. Rounding outward like
/// this makes the enumeration intervals too *wide*, never too narrow — the
/// extra candidates are thrown away by the exact test at each node, but a
/// candidate we never generated would be a silently lost solution.
fn sqrt_upper_bound(q: &Rat) -> Rat {
    debug_assert!(!q.is_negative());
    let num = ceil_sqrt(q.numer());
    // `BigRational` always keeps the denominator positive, so this is >= 1.
    let den = q.denom().sqrt();
    Rat::new(num, den)
}

/// Dense matrix of exact rationals, stored row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatMatrix {
    rows: usize,
    cols: usize,
    data: Vec<Rat>,
}

impl RatMatrix {
    /// All-zero `rows` x `cols` matrix.
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![Rat::zero(); rows * cols],
        }
    }

    /// `n` x `n` identity.
    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m.set(i, i, Rat::one());
        }
        m
    }

    /// Builds from row-major integers. Panics if `v.len() != rows * cols`;
    /// this is a construction helper, not a parser, so a mismatch is a bug in
    /// the caller rather than bad input.
    pub fn from_i64(rows: usize, cols: usize, v: &[i64]) -> Self {
        assert_eq!(
            v.len(),
            rows * cols,
            "from_i64 needs exactly rows*cols entries"
        );
        Self {
            rows,
            cols,
            data: v.iter().map(|&x| rat(x)).collect(),
        }
    }

    /// Builds from rows of rationals. Panics if the rows are ragged.
    pub fn from_rows(rows: &[Vec<Rat>]) -> Self {
        let nrows = rows.len();
        let ncols = rows.first().map_or(0, |r| r.len());
        let mut data = Vec::with_capacity(nrows * ncols);
        for r in rows {
            assert_eq!(r.len(), ncols, "from_rows needs rectangular input");
            data.extend(r.iter().cloned());
        }
        Self {
            rows: nrows,
            cols: ncols,
            data,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    #[inline]
    fn idx(&self, r: usize, c: usize) -> usize {
        debug_assert!(r < self.rows && c < self.cols);
        r * self.cols + c
    }

    pub fn get(&self, r: usize, c: usize) -> &Rat {
        &self.data[self.idx(r, c)]
    }

    pub fn set(&mut self, r: usize, c: usize, v: Rat) {
        let i = self.idx(r, c);
        self.data[i] = v;
    }

    /// Copy of row `r`.
    pub fn row(&self, r: usize) -> Vec<Rat> {
        let start = self.idx(r, 0);
        self.data[start..start + self.cols].to_vec()
    }

    /// Overwrites row `r`. Panics on a length mismatch.
    pub fn set_row(&mut self, r: usize, v: &[Rat]) {
        assert_eq!(v.len(), self.cols, "row length mismatch");
        let start = self.idx(r, 0);
        self.data[start..start + self.cols].clone_from_slice(v);
    }

    pub fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        for c in 0..self.cols {
            let (i, j) = (self.idx(a, c), self.idx(b, c));
            self.data.swap(i, j);
        }
    }

    pub fn is_zero_row(&self, r: usize) -> bool {
        (0..self.cols).all(|c| self.get(r, c).is_zero())
    }

    /// The matrix with all all-zero rows removed.
    ///
    /// After LLL on a rank-deficient input the dependent rows collapse to zero
    /// and gather at the top; dropping them leaves a genuine basis of the same
    /// lattice, which is what enumeration needs (a dependent generating set
    /// gives every lattice point infinitely many coefficient vectors, and the
    /// search would never terminate).
    pub fn drop_zero_rows(&self) -> RatMatrix {
        let kept: Vec<Vec<Rat>> = (0..self.rows)
            .filter(|&r| !self.is_zero_row(r))
            .map(|r| self.row(r))
            .collect();
        if kept.is_empty() {
            return RatMatrix::zeros(0, self.cols);
        }
        RatMatrix::from_rows(&kept)
    }

    pub fn transpose(&self) -> RatMatrix {
        let mut out = RatMatrix::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                out.set(c, r, self.get(r, c).clone());
            }
        }
        out
    }

    pub fn multiply(&self, other: &RatMatrix) -> Result<RatMatrix> {
        ensure!(
            self.cols == other.rows,
            "cannot multiply {}x{} by {}x{}",
            self.rows,
            self.cols,
            other.rows,
            other.cols
        );
        let mut out = RatMatrix::zeros(self.rows, other.cols);
        for r in 0..self.rows {
            for k in 0..self.cols {
                let a = self.get(r, k);
                if a.is_zero() {
                    continue; // the common case for sparse-ish lattice bases
                }
                for c in 0..other.cols {
                    let v = out.get(r, c) + a * other.get(k, c);
                    out.set(r, c, v);
                }
            }
        }
        Ok(out)
    }

    /// `self * v`, treating `v` as a column vector.
    pub fn multiply_vec(&self, v: &[Rat]) -> Result<Vec<Rat>> {
        ensure!(
            self.cols == v.len(),
            "cannot multiply {}x{} by a vector of length {}",
            self.rows,
            self.cols,
            v.len()
        );
        Ok((0..self.rows)
            .map(|r| {
                let start = self.idx(r, 0);
                dot(&self.data[start..start + self.cols], v)
            })
            .collect())
    }

    /// Determinant by exact Gaussian elimination. Err if not square.
    pub fn determinant(&self) -> Result<Rat> {
        ensure!(
            self.rows == self.cols,
            "determinant needs a square matrix, got {}x{}",
            self.rows,
            self.cols
        );
        let n = self.rows;
        let mut a = self.clone();
        let mut det = Rat::one();
        for col in 0..n {
            // Any nonzero pivot will do: with exact arithmetic there is no
            // stability argument for choosing a large one.
            let Some(p) = (col..n).find(|&r| !a.get(r, col).is_zero()) else {
                return Ok(Rat::zero());
            };
            if p != col {
                a.swap_rows(p, col);
                det = -det;
            }
            let pivot = a.get(col, col).clone();
            det *= &pivot;
            for r in (col + 1)..n {
                let f = a.get(r, col) / &pivot;
                if f.is_zero() {
                    continue;
                }
                for c in col..n {
                    let v = a.get(r, c) - &f * a.get(col, c);
                    a.set(r, c, v);
                }
            }
        }
        Ok(det)
    }

    /// Exact inverse by Gauss-Jordan on `[self | I]`. Err if not square or
    /// singular.
    pub fn inverse(&self) -> Result<RatMatrix> {
        ensure!(
            self.rows == self.cols,
            "inverse needs a square matrix, got {}x{}",
            self.rows,
            self.cols
        );
        let n = self.rows;
        let mut a = self.clone();
        let mut inv = RatMatrix::identity(n);

        for col in 0..n {
            let Some(p) = (col..n).find(|&r| !a.get(r, col).is_zero()) else {
                bail!("matrix is singular: no pivot in column {col}");
            };
            a.swap_rows(p, col);
            inv.swap_rows(p, col);

            // Normalise the pivot row to a leading 1.
            let pivot = a.get(col, col).clone();
            for c in 0..n {
                let v = a.get(col, c) / &pivot;
                a.set(col, c, v);
                let v = inv.get(col, c) / &pivot;
                inv.set(col, c, v);
            }

            // Clear the column everywhere else.
            for r in 0..n {
                if r == col {
                    continue;
                }
                let f = a.get(r, col).clone();
                if f.is_zero() {
                    continue;
                }
                for c in 0..n {
                    let v = a.get(r, c) - &f * a.get(col, c);
                    a.set(r, c, v);
                    let v = inv.get(r, c) - &f * inv.get(col, c);
                    inv.set(r, c, v);
                }
            }
        }
        Ok(inv)
    }
}

/// Gram-Schmidt orthogonalisation of the basis **rows**.
///
/// Returns `(b_star, mu)` where `b_star` holds the orthogonalised rows and
/// `mu` is `rows x rows` lower-triangular with `mu[k][k] == 1`, so that
///
/// ```text
/// b_k == b*_k + sum_{j<k} mu[k][j] * b*_j
/// ```
///
/// holds exactly. No normalisation is performed — `b*` rows are not unit
/// vectors, and their squared norms are what LLL and Fincke-Pohst actually
/// consume.
///
/// Linearly dependent rows are not an error: the offending `b*_k` comes out as
/// the zero vector, and `mu[k][j]` for a zero `b*_j` is defined to be 0 (the
/// true coefficient is `0/0`). The identity above still holds, because a zero
/// `b*_j` contributes nothing whatever its coefficient.
pub fn gram_schmidt(basis: &RatMatrix) -> Result<(RatMatrix, RatMatrix)> {
    let n = basis.rows();
    let m = basis.cols();
    let mut b_star = RatMatrix::zeros(n, m);
    let mut mu = RatMatrix::zeros(n, n);
    let mut norms: Vec<Rat> = vec![Rat::zero(); n];

    for k in 0..n {
        let bk = basis.row(k);
        let mut row = bk.clone();
        for (j, nj) in norms.iter().enumerate().take(k) {
            let coeff = if nj.is_zero() {
                Rat::zero()
            } else {
                dot(&bk, &b_star.row(j)) / nj
            };
            sub_scaled(&mut row, &b_star.row(j), &coeff);
            mu.set(k, j, coeff);
        }
        mu.set(k, k, Rat::one());
        norms[k] = dot(&row, &row);
        b_star.set_row(k, &row);
    }
    Ok((b_star, mu))
}

/// Output of [`lll_reduce`].
pub struct Reduced {
    /// The reduced basis, again as rows.
    pub basis: RatMatrix,
    /// Unimodular `T` with `T * original == basis`.
    pub transform: RatMatrix,
}

/// LLL reduction of the lattice spanned by the **rows** of `basis`.
///
/// `delta` must lie in `(1/4, 1)`; `3/4` is the classic choice and what
/// LattiCG uses. Larger `delta` gives a better-reduced basis for more work.
///
/// Unlike LattiCG this keeps zero rows in the output instead of trimming them,
/// so `transform` stays square and unimodular and `transform * original` is
/// exactly `basis`. Callers that want a genuine basis of a rank-deficient
/// input should follow up with [`RatMatrix::drop_zero_rows`] — LLL leaves the
/// zero rows at the top.
pub fn lll_reduce(basis: &RatMatrix, delta: &Rat) -> Result<Reduced> {
    ensure!(
        *delta > ratio(1, 4) && *delta < Rat::one(),
        "delta must lie strictly between 1/4 and 1, got {delta}"
    );
    let mut run = LllRun::new(basis);
    run.reduce(delta);
    Ok(Reduced {
        basis: run.basis,
        transform: run.transform,
    })
}

/// Working state for Cohen's algorithm 2.6.3 (the "integral" bookkeeping is
/// skipped — we are exact anyway — but the incremental GSO updates are not:
/// recomputing Gram-Schmidt from scratch after every swap is what turns an
/// exact-rational LLL from slow into unusable).
struct LllRun {
    n: usize,
    basis: RatMatrix,
    transform: RatMatrix,
    /// Gram-Schmidt vectors of the *current* basis.
    gso: RatMatrix,
    /// `mu[k][j]`, kept in step with `gso` through swaps.
    mu: RatMatrix,
    /// `norms[k] == |b*_k|^2`.
    norms: Vec<Rat>,
}

impl LllRun {
    fn new(basis: &RatMatrix) -> Self {
        let n = basis.rows();
        Self {
            n,
            basis: basis.clone(),
            transform: RatMatrix::identity(n),
            gso: RatMatrix::zeros(n, basis.cols()),
            mu: RatMatrix::zeros(n, n),
            norms: vec![Rat::zero(); n],
        }
    }

    /// Computes `b*_k` and `mu[k][0..k]` from scratch. Only ever called the
    /// first time row `k` comes into play; after that the GSO is maintained by
    /// [`LllRun::swap`].
    fn update_gso(&mut self, k: usize) {
        let bk = self.basis.row(k);
        let mut row = bk.clone();
        for j in 0..k {
            let coeff = if self.norms[j].is_zero() {
                Rat::zero()
            } else {
                dot(&bk, &self.gso.row(j)) / &self.norms[j]
            };
            sub_scaled(&mut row, &self.gso.row(j), &coeff);
            self.mu.set(k, j, coeff);
        }
        self.mu.set(k, k, Rat::one());
        self.norms[k] = dot(&row, &row);
        self.gso.set_row(k, &row);
    }

    /// Cohen's `RED(i, j)`: subtract the nearest integer multiple of row `j`
    /// from row `i`, which drags `mu[i][j]` into `[-1/2, 1/2]`.
    ///
    /// This changes the basis but not the lattice, and not the GSO either —
    /// `b*_i` is unchanged because we only added a multiple of an *earlier*
    /// row, which lives entirely in the span `b*_i` was projected out of. Only
    /// the `mu` entries need fixing up.
    fn red(&mut self, i: usize, j: usize) {
        let q = self.mu.get(i, j).round().to_integer();
        if q.is_zero() {
            return; // |mu[i][j]| < 1/2 already
        }
        let qr = Rat::from_integer(q);

        let bj = self.basis.row(j);
        let mut bi = self.basis.row(i);
        sub_scaled(&mut bi, &bj, &qr);
        self.basis.set_row(i, &bi);

        let hj = self.transform.row(j);
        let mut hi = self.transform.row(i);
        sub_scaled(&mut hi, &hj, &qr);
        self.transform.set_row(i, &hi);

        let v = self.mu.get(i, j) - &qr;
        self.mu.set(i, j, v);
        for c in 0..j {
            let v = self.mu.get(i, c) - &qr * self.mu.get(j, c);
            self.mu.set(i, c, v);
        }
    }

    /// Lovasz test: true when the condition *fails* and rows `k`, `k-1` should
    /// be swapped.
    fn lovasz_fails(&self, k: usize, delta: &Rat) -> bool {
        let m = self.mu.get(k, k - 1);
        let factor = delta - m * m;
        self.norms[k] < &self.norms[k - 1] * &factor
    }

    /// Cohen's `SWAP(k)`: exchange rows `k` and `k-1` and patch the GSO in
    /// place.
    ///
    /// The patch is the only subtle part of LLL. Writing `t = mu[k][k-1]`, the
    /// new `b*_{k-1}` is `b*_k + t*b*_{k-1}` (the old row `k` projected past
    /// only the first `k-1` rows), whose squared norm is
    /// `B_k + t^2*B_{k-1} =: tB`. The new `b*_k` is the old `b*_{k-1}` with the
    /// new `b*_{k-1}` direction projected out. The product `B_{k-1}*B_k` is
    /// invariant (it is a sub-determinant), which is why the new norms come out
    /// as `tB` and `B_k*B_{k-1}/tB`.
    fn swap(&mut self, k: usize, kmax: usize) {
        self.basis.swap_rows(k, k - 1);
        self.transform.swap_rows(k, k - 1);

        // Columns 0..k-2 of the two rows just follow their rows.
        for j in 0..k.saturating_sub(1) {
            let a = self.mu.get(k, j).clone();
            let b = self.mu.get(k - 1, j).clone();
            self.mu.set(k, j, b);
            self.mu.set(k - 1, j, a);
        }

        let tmu = self.mu.get(k, k - 1).clone();
        let tb = &self.norms[k] + &(&tmu * &tmu) * &self.norms[k - 1];

        if tb.is_zero() {
            // Both rows are dependent on the earlier ones. Push the zero
            // Gram-Schmidt vector down; it will end up at the top of the basis
            // as a genuine zero row.
            self.norms[k] = self.norms[k - 1].clone();
            self.norms[k - 1] = Rat::zero();
            self.gso.swap_rows(k, k - 1);
            for i in (k + 1)..=kmax {
                let v = self.mu.get(i, k - 1).clone();
                self.mu.set(i, k, v);
                self.mu.set(i, k - 1, Rat::zero());
            }
        } else if self.norms[k].is_zero() && !tmu.is_zero() {
            // Old row k was dependent but the swap makes the pair usable: the
            // new b*_{k-1} is just t times the old one.
            self.norms[k - 1] = tb;
            let scaled: Vec<Rat> = self.gso.row(k - 1).iter().map(|x| x * &tmu).collect();
            self.gso.set_row(k - 1, &scaled);
            self.mu.set(k, k - 1, Rat::one() / &tmu);
            for i in (k + 1)..=kmax {
                let v = self.mu.get(i, k - 1) / &tmu;
                self.mu.set(i, k - 1, v);
            }
        } else {
            let t = &self.norms[k - 1] / &tb;
            let new_mu = &tmu * &t;
            self.mu.set(k, k - 1, new_mu.clone());

            let old_km1 = self.gso.row(k - 1);
            let old_k = self.gso.row(k);
            let shrink = &self.norms[k] / &tb;
            let mut row_km1 = Vec::with_capacity(old_k.len());
            let mut row_k = Vec::with_capacity(old_k.len());
            for c in 0..old_k.len() {
                row_km1.push(&old_k[c] + &tmu * &old_km1[c]);
                row_k.push(&shrink * &old_km1[c] - &new_mu * &old_k[c]);
            }
            self.gso.set_row(k - 1, &row_km1);
            self.gso.set_row(k, &row_k);

            let nk = &self.norms[k] * &t;
            self.norms[k] = nk;
            self.norms[k - 1] = tb;

            // Rows above the swap see both coefficients change.
            for i in (k + 1)..=kmax {
                let old_ik = self.mu.get(i, k).clone();
                let new_ik = self.mu.get(i, k - 1) - &tmu * &old_ik;
                let new_ikm1 = &old_ik + &new_mu * &new_ik;
                self.mu.set(i, k, new_ik);
                self.mu.set(i, k - 1, new_ikm1);
            }
        }
    }

    fn reduce(&mut self, delta: &Rat) {
        if self.n == 0 {
            return;
        }
        let b0 = self.basis.row(0);
        self.norms[0] = dot(&b0, &b0);
        self.gso.set_row(0, &b0);
        self.mu.set(0, 0, Rat::one());

        let mut k = 1usize;
        let mut kmax = 0usize;
        // Row k's GSO is only computed the first time k reaches a new high;
        // after a swap the GSO is already correct, so recomputing would be both
        // wasted work and (with `kmax` unchanged) wrong bookkeeping.
        let mut fresh = true;

        while k < self.n {
            if k > kmax && fresh {
                kmax = k;
                self.update_gso(k);
            }
            self.red(k, k - 1);
            if self.lovasz_fails(k, delta) {
                self.swap(k, kmax);
                k = if k > 1 { k - 1 } else { 1 };
                fresh = false;
            } else {
                // Full size reduction against every earlier row. Doing this
                // only on the way *up* is what keeps the coefficients small
                // without redundant work after each swap.
                for l in (0..k.saturating_sub(1)).rev() {
                    self.red(k, l);
                }
                k += 1;
                fresh = true;
            }
        }
    }
}

/// Node budget for [`enumerate_in_box`] before it gives up.
///
/// A Fincke-Pohst node is one candidate coefficient tried at one level of the
/// tree, so this is roughly "16 million integer choices". A well-conditioned
/// problem with a sane box uses a tiny fraction of it; blowing through it means
/// the caller handed us a box that is enormous relative to the lattice, or a
/// basis so skewed that LLL could not save it. Failing is better than hanging.
///
/// This is a backstop, not a timeout. A node costs a handful of exact-rational
/// operations on numbers that grow with the lattice, so actually exhausting the
/// budget takes minutes, not milliseconds. Most hopeless searches are rejected
/// far sooner: a single level whose candidate interval alone overruns the
/// remaining budget is refused without descending into it. Callers who want a
/// tighter leash should use [`enumerate_in_box_capped`].
pub const DEFAULT_MAX_NODES: u64 = 1 << 24;

/// Every lattice point `x` with `lower[i] <= x[i] + offset[i] <= upper[i]`.
///
/// The rows of `basis` generate the lattice; the returned vectors are the
/// ambient points `x` (integer combinations of those rows), **not** the
/// coefficient vectors. Output is sorted lexicographically so results are
/// reproducible.
///
/// # Method
///
/// The box is contained in the ball centred on the box centre (minus `offset`)
/// with radius `R`, where `R^2 = sum_i ((upper[i]-lower[i])/2)^2` — that is
/// exactly the corner distance. So enumerating the closed ball and filtering to
/// the box is complete, and completeness is the only property that matters
/// here: a subtly wrong bound loses solutions without saying so.
///
/// The ball is enumerated by classical Fincke-Pohst over an LLL-reduced basis.
/// LattiCG instead tightens the bounds at each branch with an exact-rational
/// simplex LP, which prunes harder; that is deliberately not ported, because
/// an LP is a great deal of machinery to get provably right and this module's
/// value is in being provably right. The price is a wider search tree.
///
/// # Scaling
///
/// No internal scaling is done. If your box is wildly anisotropic — say
/// 2^32 wide on one axis and 16 on another — the ball is dominated by the long
/// axis and the enumeration explores far more of the lattice than it needs to.
/// Scale each axis yourself (multiply column `i` of the basis, and `offset`,
/// `lower`, `upper` entry `i`, by a common factor such as `lcm/side_length[i]`,
/// the way LattiCG does) so the box is roughly a cube, then undo the scaling on
/// the results.
///
/// # Errors
///
/// Dimension mismatch, a rank-deficient basis that survives zero-row removal,
/// or exceeding [`DEFAULT_MAX_NODES`] nodes.
pub fn enumerate_in_box(
    basis: &RatMatrix,
    offset: &[Rat],
    lower: &[Rat],
    upper: &[Rat],
) -> Result<Vec<Vec<Rat>>> {
    enumerate_in_box_capped(basis, offset, lower, upper, DEFAULT_MAX_NODES)
}

/// [`enumerate_in_box`] with an explicit node budget. See [`DEFAULT_MAX_NODES`].
pub fn enumerate_in_box_capped(
    basis: &RatMatrix,
    offset: &[Rat],
    lower: &[Rat],
    upper: &[Rat],
    max_nodes: u64,
) -> Result<Vec<Vec<Rat>>> {
    let m = basis.cols();
    ensure!(
        offset.len() == m && lower.len() == m && upper.len() == m,
        "offset/lower/upper must all have length {m} (the ambient dimension)"
    );

    // Fold the offset into the box: lower <= x + offset <= upper becomes
    // lo <= x <= hi. Everything downstream works on x directly.
    let lo: Vec<Rat> = (0..m).map(|i| &lower[i] - &offset[i]).collect();
    let hi: Vec<Rat> = (0..m).map(|i| &upper[i] - &offset[i]).collect();
    if (0..m).any(|i| lo[i] > hi[i]) {
        return Ok(Vec::new()); // empty box, nothing to search
    }

    // Centre and squared circumradius of the box.
    let two = rat(2);
    let centre: Vec<Rat> = (0..m).map(|i| (&lo[i] + &hi[i]) / &two).collect();
    let mut r2 = Rat::zero();
    for i in 0..m {
        let h = (&hi[i] - &lo[i]) / &two;
        r2 += &h * &h;
    }

    // A reduced basis is what makes the tree small; without it Fincke-Pohst on
    // a skewed basis degenerates into scanning a huge parallelepiped.
    let reduced = lll_reduce(basis, &ratio(3, 4))?.basis.drop_zero_rows();
    let n = reduced.rows();

    if n == 0 {
        // The lattice is {0}; the origin is the only candidate.
        let origin = vec![Rat::zero(); m];
        let inside = (0..m).all(|i| lo[i] <= origin[i] && origin[i] <= hi[i]);
        return Ok(if inside { vec![origin] } else { Vec::new() });
    }

    let (b_star, mu) = gram_schmidt(&reduced)?;
    let mut norms = Vec::with_capacity(n);
    for j in 0..n {
        let row = b_star.row(j);
        let nj = dot(&row, &row);
        ensure!(
            !nj.is_zero(),
            "basis is rank-deficient even after LLL; cannot enumerate"
        );
        norms.push(nj);
    }

    // Split the centre into the part inside the lattice's span and the part
    // orthogonal to it. Only the in-span part can ever be approached by a
    // lattice point, so the orthogonal part is a fixed toll paid out of the
    // radius budget up front.
    let mut tau = Vec::with_capacity(n);
    let mut par2 = Rat::zero();
    for (j, nj) in norms.iter().enumerate() {
        let t = dot(&centre, &b_star.row(j)) / nj;
        par2 += &t * &t * nj;
        tau.push(t);
    }
    let perp2 = dot(&centre, &centre) - par2;
    let budget = &r2 - &perp2;
    if budget.is_negative() {
        return Ok(Vec::new()); // the whole span is further than R away
    }

    let mut enumerator = Enumerator {
        basis: &reduced,
        mu: &mu,
        norms: &norms,
        tau: &tau,
        lo: &lo,
        hi: &hi,
        z: vec![BigInt::zero(); n],
        nodes: 0,
        max_nodes,
        out: Vec::new(),
    };
    enumerator.search(n, budget)?;

    let mut out = enumerator.out;
    out.sort();
    Ok(out)
}

/// Depth-first Fincke-Pohst search over coefficient vectors.
struct Enumerator<'a> {
    basis: &'a RatMatrix,
    mu: &'a RatMatrix,
    norms: &'a [Rat],
    /// Coordinates of the ball centre in the `b*` basis.
    tau: &'a [Rat],
    lo: &'a [Rat],
    hi: &'a [Rat],
    z: Vec<BigInt>,
    nodes: u64,
    max_nodes: u64,
    out: Vec<Vec<Rat>>,
}

impl Enumerator<'_> {
    /// `level` counts down from `n`; `level == 0` is a complete coefficient
    /// vector. `remaining` is the squared radius still unspent.
    ///
    /// The identity being exploited: with `b_i = sum_{j<=i} mu[i][j] b*_j`, the
    /// `b*_j` component of `sum_i z_i b_i` is `z_j + sum_{i>j} z_i mu[i][j]`.
    /// Because the `b*` are orthogonal, the squared distance to the centre
    /// splits into one independent term per level, and the term for level `j`
    /// depends only on `z_j..z_{n-1}`. That is what makes the depth-first
    /// pruning valid.
    fn search(&mut self, level: usize, remaining: Rat) -> Result<()> {
        if level == 0 {
            self.emit();
            return Ok(());
        }
        let j = level - 1;

        // Contribution of the already-fixed higher coefficients to the b*_j
        // component.
        let mut fixed = Rat::zero();
        for i in (j + 1)..self.z.len() {
            if self.z[i].is_zero() {
                continue;
            }
            fixed += Rat::from_integer(self.z[i].clone()) * self.mu.get(i, j);
        }
        let centre = &self.tau[j] - &fixed;

        // |z_j - centre|^2 * B_j <= remaining. The square root is bounded
        // outward, so this interval is a superset of the true one.
        let width = sqrt_upper_bound(&(&remaining / &self.norms[j]));
        let first = (&centre - &width).ceil().to_integer();
        let last = (&centre + &width).floor().to_integer();
        if first > last {
            return Ok(());
        }

        // Refuse a level that alone would exhaust the budget, rather than
        // grinding through it one BigInt at a time.
        let span = &last - &first + 1;
        if span > BigInt::from(self.max_nodes - self.nodes) {
            bail!(
                "lattice enumeration exceeded its node budget of {} \
                 (level {j} alone spans {span} candidates); \
                 the box is too large for this lattice, or needs per-axis scaling",
                self.max_nodes
            );
        }

        let mut zj = first;
        while zj <= last {
            self.nodes += 1;
            if self.nodes > self.max_nodes {
                bail!(
                    "lattice enumeration exceeded its node budget of {}",
                    self.max_nodes
                );
            }
            // Exact test — the interval above was deliberately generous, so
            // this is where over-generated candidates get dropped.
            let d = &(Rat::from_integer(zj.clone()) + &fixed) - &self.tau[j];
            let cost = &d * &d * &self.norms[j];
            if cost <= remaining {
                self.z[j] = zj.clone();
                let rest = &remaining - &cost;
                self.search(level - 1, rest)?;
            }
            zj += 1;
        }
        Ok(())
    }

    /// Turns the current coefficient vector into an ambient point and keeps it
    /// if it is really in the box (the ball is a superset).
    fn emit(&mut self) {
        let m = self.basis.cols();
        let mut x = vec![Rat::zero(); m];
        for (i, zi) in self.z.iter().enumerate() {
            if zi.is_zero() {
                continue;
            }
            let zr = Rat::from_integer(zi.clone());
            for (c, xc) in x.iter_mut().enumerate() {
                *xc += &zr * self.basis.get(i, c);
            }
        }
        if (0..m).all(|c| self.lo[c] <= x[c] && x[c] <= self.hi[c]) {
            self.out.push(x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn mat(rows: usize, cols: usize, v: &[i64]) -> RatMatrix {
        RatMatrix::from_i64(rows, cols, v)
    }

    fn rats(v: &[i64]) -> Vec<Rat> {
        v.iter().map(|&x| rat(x)).collect()
    }

    /// xorshift64*, purely so the "random-ish" cases are reproducible across
    /// runs and machines. Not used for anything but test-case generation.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        /// Uniform-ish integer in `[lo, hi]`.
        fn range(&mut self, lo: i64, hi: i64) -> i64 {
            let span = (hi - lo + 1) as u64;
            lo + (self.next_u64() % span) as i64
        }
    }

    // ---- RatMatrix ------------------------------------------------------

    #[test]
    fn identity_is_a_multiplicative_unit() {
        let a = mat(2, 3, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(RatMatrix::identity(2).multiply(&a).unwrap(), a);
        assert_eq!(a.multiply(&RatMatrix::identity(3)).unwrap(), a);
    }

    #[test]
    fn multiply_matches_hand_computation() {
        let a = mat(2, 3, &[1, 2, 3, 4, 5, 6]);
        let b = mat(3, 2, &[7, 8, 9, 10, 11, 12]);
        let c = a.multiply(&b).unwrap();
        assert_eq!(c.rows(), 2);
        assert_eq!(c.cols(), 2);
        assert_eq!(*c.get(0, 0), rat(58));
        assert_eq!(*c.get(0, 1), rat(64));
        assert_eq!(*c.get(1, 0), rat(139));
        assert_eq!(*c.get(1, 1), rat(154));
        assert!(b.multiply(&a).unwrap().rows() == 3);
        assert!(a.multiply(&a).is_err());
    }

    #[test]
    fn transpose_round_trips_and_commutes_with_multiply() {
        let a = mat(2, 3, &[1, -2, 3, 4, 5, -6]);
        assert_eq!(a.transpose().transpose(), a);
        let b = mat(3, 2, &[7, 8, -9, 10, 11, 12]);
        // (AB)^T == B^T A^T
        let lhs = a.multiply(&b).unwrap().transpose();
        let rhs = b.transpose().multiply(&a.transpose()).unwrap();
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn multiply_vec_agrees_with_matrix_multiply() {
        let a = mat(2, 3, &[1, 2, 3, 4, 5, 6]);
        let v = rats(&[1, -1, 2]);
        let got = a.multiply_vec(&v).unwrap();
        assert_eq!(got, rats(&[5, 11]));
        assert!(a.multiply_vec(&rats(&[1, 2])).is_err());
    }

    #[test]
    fn inverse_times_original_is_identity() {
        let fixed = [
            (2, vec![1i64, 2, 3, 4]),
            (2, vec![0, 1, 1, 0]),
            (3, vec![2, 0, 1, 1, 3, 2, 1, 1, 4]),
            (3, vec![1, 1, 1, -1, 0, 2, 3, 5, 6]),
            (4, vec![1, 0, 0, 2, 0, 3, 1, 0, 4, 0, 1, 1, 0, 2, 0, 5]),
        ];
        for (n, v) in fixed {
            let a = mat(n, n, &v);
            let inv = a.inverse().unwrap();
            assert_eq!(a.multiply(&inv).unwrap(), RatMatrix::identity(n));
            assert_eq!(inv.multiply(&a).unwrap(), RatMatrix::identity(n));
        }

        // And a batch of pseudo-random integer matrices.
        let mut rng = Rng(0x1234_5678_9abc_def0);
        let mut checked = 0;
        for _ in 0..40 {
            let n = rng.range(1, 4) as usize;
            let v: Vec<i64> = (0..n * n).map(|_| rng.range(-5, 5)).collect();
            let a = mat(n, n, &v);
            if a.determinant().unwrap().is_zero() {
                assert!(a.inverse().is_err(), "singular matrix must not invert");
                continue;
            }
            let inv = a.inverse().unwrap();
            assert_eq!(a.multiply(&inv).unwrap(), RatMatrix::identity(n));
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} invertible cases generated");
    }

    #[test]
    fn singular_matrices_do_not_invert() {
        // Second row is twice the first.
        assert!(mat(2, 2, &[1, 2, 2, 4]).inverse().is_err());
        // Rank 2 in a 3x3.
        assert!(mat(3, 3, &[1, 2, 3, 2, 4, 6, 0, 1, 1]).inverse().is_err());
        // A zero row.
        assert!(mat(2, 2, &[0, 0, 1, 1]).inverse().is_err());
        // Not square.
        assert!(mat(2, 3, &[1, 0, 0, 0, 1, 0]).inverse().is_err());
    }

    #[test]
    fn determinant_matches_known_values() {
        assert_eq!(mat(2, 2, &[1, 2, 3, 4]).determinant().unwrap(), rat(-2));
        assert_eq!(
            mat(3, 3, &[1, 1, 1, -1, 0, 2, 3, 5, 6]).determinant().unwrap(),
            rat(-3)
        );
        assert_eq!(RatMatrix::identity(5).determinant().unwrap(), rat(1));
        assert!(mat(2, 2, &[1, 2, 2, 4]).determinant().unwrap().is_zero());
    }

    // ---- Gram-Schmidt ---------------------------------------------------

    fn gs_cases() -> Vec<RatMatrix> {
        vec![
            mat(2, 2, &[1, 1, 2, 0]),
            mat(3, 3, &[1, 1, 1, -1, 0, 2, 3, 5, 6]),
            mat(3, 4, &[1, 0, 2, 1, 3, 1, 0, -2, 0, 4, 1, 1]),
            mat(4, 4, &[2, 0, 0, 1, 1, 3, 0, 0, 0, 1, 5, 2, 7, 1, 1, 1]),
            // Rank-deficient: row 2 = row 0 + row 1.
            mat(3, 3, &[1, 2, 3, 4, 5, 6, 5, 7, 9]),
        ]
    }

    #[test]
    fn gram_schmidt_vectors_are_pairwise_orthogonal() {
        for basis in gs_cases() {
            let (bs, _) = gram_schmidt(&basis).unwrap();
            for i in 0..bs.rows() {
                for j in 0..i {
                    assert!(
                        dot(&bs.row(i), &bs.row(j)).is_zero(),
                        "b*_{i} . b*_{j} is not exactly zero"
                    );
                }
            }
        }
    }

    #[test]
    fn gram_schmidt_reconstructs_the_basis() {
        for basis in gs_cases() {
            let (bs, mu) = gram_schmidt(&basis).unwrap();
            for k in 0..basis.rows() {
                let mut recon = bs.row(k);
                for j in 0..k {
                    let coeff = mu.get(k, j).clone();
                    let bstar_j = bs.row(j);
                    for c in 0..recon.len() {
                        recon[c] += &coeff * &bstar_j[c];
                    }
                }
                assert_eq!(recon, basis.row(k), "row {k} does not reconstruct");
                assert_eq!(*mu.get(k, k), Rat::one(), "mu diagonal must be 1");
            }
        }
    }

    // ---- LLL ------------------------------------------------------------

    /// Asserts the two defining LLL properties on a basis of independent rows.
    fn assert_lll_reduced(basis: &RatMatrix, delta: &Rat) {
        let (bs, mu) = gram_schmidt(basis).unwrap();
        let n = basis.rows();
        let half = ratio(1, 2);
        for k in 0..n {
            for j in 0..k {
                assert!(
                    mu.get(k, j).abs() <= half,
                    "size reduction failed: |mu[{k}][{j}]| = {} > 1/2",
                    mu.get(k, j).abs()
                );
            }
        }
        for k in 1..n {
            let bk = dot(&bs.row(k), &bs.row(k));
            let bk1 = dot(&bs.row(k - 1), &bs.row(k - 1));
            let m = mu.get(k, k - 1);
            let need = (delta - m * m) * &bk1;
            assert!(bk >= need, "Lovasz condition fails at k = {k}");
        }
    }

    fn lll_cases() -> Vec<RatMatrix> {
        vec![
            mat(2, 2, &[1, 1, 2, 0]),
            mat(2, 2, &[201, 37, 1648, 297]),
            mat(3, 3, &[1, 1, 1, -1, 0, 2, 3, 5, 6]),
            mat(3, 3, &[105, 821, 404, 324, 1002, 8, 121, 33, 723]),
            mat(3, 4, &[1, 0, 2, 1, 3, 1, 0, -2, 0, 4, 1, 1]),
            mat(4, 4, &[2, 0, 0, 1, 1, 3, 0, 0, 0, 1, 5, 2, 7, 1, 1, 1]),
            mat(
                5,
                5,
                &[
                    1, 0, 0, 0, 12345, 0, 1, 0, 0, 6789, 0, 0, 1, 0, 111, 0, 0, 0, 1, 2222, 0, 0,
                    0, 0, 65536,
                ],
            ),
        ]
    }

    #[test]
    fn lll_transform_is_unimodular_and_consistent() {
        let delta = ratio(3, 4);
        for basis in lll_cases() {
            let out = lll_reduce(&basis, &delta).unwrap();
            let det = out.transform.determinant().unwrap();
            assert!(
                det.abs() == Rat::one(),
                "transform determinant is {det}, not +-1"
            );
            assert_eq!(
                out.transform.multiply(&basis).unwrap(),
                out.basis,
                "transform * original != reduced"
            );
        }
    }

    #[test]
    fn lll_output_satisfies_the_lll_conditions() {
        for delta in [ratio(3, 4), ratio(51, 100), ratio(99, 100)] {
            for basis in lll_cases() {
                let out = lll_reduce(&basis, &delta).unwrap();
                assert_lll_reduced(&out.basis, &delta);
            }
        }
    }

    #[test]
    fn lll_preserves_determinant_magnitude() {
        let delta = ratio(3, 4);
        for basis in lll_cases() {
            if basis.rows() != basis.cols() {
                continue; // determinant is only defined for the square ones
            }
            let out = lll_reduce(&basis, &delta).unwrap();
            assert_eq!(
                out.basis.determinant().unwrap().abs(),
                basis.determinant().unwrap().abs()
            );
        }
    }

    #[test]
    fn lll_reduces_the_classic_small_case() {
        let basis = mat(3, 3, &[1, 1, 1, -1, 0, 2, 3, 5, 6]);
        let delta = ratio(3, 4);
        let out = lll_reduce(&basis, &delta).unwrap();
        assert_lll_reduced(&out.basis, &delta);
        assert_eq!(out.transform.multiply(&basis).unwrap(), out.basis);
        assert_eq!(out.transform.determinant().unwrap().abs(), Rat::one());
        // The reduced vectors should be short: every row no longer than the
        // longest original one.
        let worst = (0..3)
            .map(|r| dot(&basis.row(r), &basis.row(r)))
            .max()
            .unwrap();
        for r in 0..3 {
            let len = dot(&out.basis.row(r), &out.basis.row(r));
            assert!(len <= worst, "row {r} got longer, not shorter");
        }
    }

    #[test]
    fn lll_handles_rank_deficient_input() {
        // Row 2 = row 0 + row 1, so the lattice is really 2-dimensional.
        let basis = mat(3, 3, &[1, 2, 3, 4, 5, 6, 5, 7, 9]);
        let delta = ratio(3, 4);
        let out = lll_reduce(&basis, &delta).unwrap();
        assert_eq!(out.transform.multiply(&basis).unwrap(), out.basis);
        assert_eq!(out.transform.determinant().unwrap().abs(), Rat::one());
        assert!(out.basis.is_zero_row(0), "zero rows should collect at the top");
        let trimmed = out.basis.drop_zero_rows();
        assert_eq!(trimmed.rows(), 2);
        assert_lll_reduced(&trimmed, &delta);
    }

    #[test]
    fn lll_rejects_bad_delta() {
        let basis = mat(2, 2, &[1, 0, 0, 1]);
        assert!(lll_reduce(&basis, &ratio(1, 4)).is_err());
        assert!(lll_reduce(&basis, &Rat::one()).is_err());
        assert!(lll_reduce(&basis, &rat(2)).is_err());
        assert!(lll_reduce(&basis, &ratio(3, 4)).is_ok());
    }

    #[test]
    fn lll_of_an_empty_basis_is_empty() {
        let basis = RatMatrix::zeros(0, 3);
        let out = lll_reduce(&basis, &ratio(3, 4)).unwrap();
        assert_eq!(out.basis.rows(), 0);
        assert_eq!(out.transform.rows(), 0);
    }

    // ---- Enumeration ----------------------------------------------------

    /// Exhaustive scan over every coefficient vector that could *possibly*
    /// land in the box, for a square full-rank basis.
    ///
    /// The range is not guessed, which is what makes this an oracle rather
    /// than a second heuristic: from `x = z * B` we get `z = x * B^-1`, so
    /// `z_j = sum_i x_i * Binv[i][j]` is a linear functional on the box, and a
    /// linear functional on a box attains its extremes at a corner. Summing
    /// the per-axis min and max of each term therefore gives exactly the
    /// interval `z_j` can occupy — no lattice point in the box can escape it.
    ///
    /// Returns `None` if the resulting scan would be too large to be worth
    /// running (which happens for near-singular bases).
    fn brute_force(
        basis: &RatMatrix,
        offset: &[Rat],
        lower: &[Rat],
        upper: &[Rat],
        budget: u64,
    ) -> Option<Vec<Vec<Rat>>> {
        let n = basis.rows();
        let m = basis.cols();
        assert_eq!(n, m, "brute force oracle only handles square bases");
        let lo: Vec<Rat> = (0..m).map(|i| &lower[i] - &offset[i]).collect();
        let hi: Vec<Rat> = (0..m).map(|i| &upper[i] - &offset[i]).collect();
        if (0..m).any(|i| lo[i] > hi[i]) {
            return Some(Vec::new());
        }
        let inv = basis.inverse().ok()?;

        let mut first = Vec::with_capacity(n);
        let mut span = Vec::with_capacity(n);
        let mut total: u64 = 1;
        for j in 0..n {
            let (mut zlo, mut zhi) = (Rat::zero(), Rat::zero());
            for i in 0..m {
                let a = &lo[i] * inv.get(i, j);
                let b = &hi[i] * inv.get(i, j);
                if a < b {
                    zlo += a;
                    zhi += b;
                } else {
                    zlo += b;
                    zhi += a;
                }
            }
            // Widened by a few units on each side purely as belt and braces:
            // the bound above is provably tight, but an oracle that is wrong
            // in the same direction as the code it checks proves nothing.
            let f = zlo.ceil().to_integer().to_string().parse::<i64>().ok()? - 3;
            let l = zhi.floor().to_integer().to_string().parse::<i64>().ok()? + 3;
            if f > l {
                return Some(Vec::new());
            }
            first.push(f);
            span.push(l - f);
            total = total.checked_mul((l - f + 1) as u64)?;
            if total > budget {
                return None;
            }
        }

        // Mixed-radix walk over the product of the per-coordinate intervals.
        let mut out = Vec::new();
        let mut counter = vec![0i64; n];
        loop {
            let mut x = vec![Rat::zero(); m];
            for j in 0..n {
                let z = rat(counter[j] + first[j]);
                if z.is_zero() {
                    continue;
                }
                for (c, xc) in x.iter_mut().enumerate() {
                    *xc += &z * basis.get(j, c);
                }
            }
            if (0..m).all(|c| lo[c] <= x[c] && x[c] <= hi[c]) {
                out.push(x);
            }

            let mut level = 0;
            loop {
                if level == n {
                    out.sort();
                    return Some(out);
                }
                counter[level] += 1;
                if counter[level] <= span[level] {
                    break;
                }
                counter[level] = 0;
                level += 1;
            }
        }
    }

    /// The single most important test in the module: enumeration must agree
    /// with an independent exhaustive scan, exactly, as a set. An
    /// implementation that is subtly wrong loses points rather than inventing
    /// them, so equality (not containment) is what is asserted.
    #[test]
    fn enumeration_matches_brute_force() {
        let mut rng = Rng(0xdead_beef_cafe_1234);
        let mut cases = 0usize;
        let mut skipped = 0usize;
        let mut points_found = 0usize;

        // Fixed 2-D and 3-D lattices with hand-picked boxes, then randomised
        // ones on top.
        let fixed_2d: Vec<Vec<i64>> = vec![
            vec![1, 0, 0, 1],
            vec![2, 0, 0, 3],
            vec![1, 2, 3, 4],
            vec![5, 1, 1, 5],
            vec![3, -1, 1, 4],
            vec![1, 0, 7, 1],
        ];
        let fixed_3d: Vec<Vec<i64>> = vec![
            vec![1, 0, 0, 0, 1, 0, 0, 0, 1],
            vec![2, 1, 0, 0, 3, 1, 1, 0, 4],
            vec![1, 1, 1, -1, 0, 2, 3, 5, 6],
            vec![4, 0, 0, 0, 4, 0, 1, 1, 2],
        ];

        let run = |basis: &RatMatrix,
                       offset: &[Rat],
                       lower: &[Rat],
                       upper: &[Rat],
                       cases: &mut usize,
                       skipped: &mut usize,
                       points: &mut usize| {
            let Some(want) = brute_force(basis, offset, lower, upper, 400_000) else {
                *skipped += 1;
                return;
            };
            let got = enumerate_in_box(basis, offset, lower, upper).unwrap();
            assert_eq!(
                got,
                want,
                "mismatch for basis {:?} box [{:?}, {:?}] offset {:?}",
                basis.data, lower, upper, offset
            );
            *cases += 1;
            *points += got.len();
        };

        for v in &fixed_2d {
            let basis = mat(2, 2, v);
            for (l, u) in [
                (vec![-3, -3], vec![3, 3]),
                (vec![0, 0], vec![0, 0]),
                (vec![-1, 5], vec![6, 9]),
                (vec![-10, -10], vec![-4, 2]),
                (vec![2, 2], vec![2, 2]),
                (vec![-7, 0], vec![7, 1]),
            ] {
                for off in [vec![0, 0], vec![1, -2]] {
                    run(
                        &basis,
                        &rats(&off),
                        &rats(&l),
                        &rats(&u),
                        &mut cases,
                        &mut skipped,
                        &mut points_found,
                    );
                }
            }
        }

        for v in &fixed_3d {
            let basis = mat(3, 3, v);
            for (l, u) in [
                (vec![-3, -3, -3], vec![3, 3, 3]),
                (vec![0, 0, 0], vec![5, 5, 5]),
                (vec![-2, -6, 1], vec![4, 0, 7]),
                (vec![1, 1, 1], vec![1, 1, 1]),
            ] {
                run(
                    &basis,
                    &rats(&[0, 0, 0]),
                    &rats(&l),
                    &rats(&u),
                    &mut cases,
                    &mut skipped,
                    &mut points_found,
                );
            }
        }

        // Randomised 2-D cases, including half-integer offsets so the rational
        // path is exercised rather than just the integer one.
        for _ in 0..150 {
            let v: Vec<i64> = (0..4).map(|_| rng.range(-4, 4)).collect();
            let basis = mat(2, 2, &v);
            if basis.determinant().unwrap().is_zero() {
                continue;
            }
            let lower: Vec<Rat> = (0..2).map(|_| rat(rng.range(-6, 6))).collect();
            let upper: Vec<Rat> = (0..2)
                .map(|i| &lower[i] + rat(rng.range(0, 8)))
                .collect();
            let offset: Vec<Rat> = (0..2).map(|_| ratio(rng.range(-6, 6), 2)).collect();
            run(
                &basis,
                &offset,
                &lower,
                &upper,
                &mut cases,
                &mut skipped,
                &mut points_found,
            );
        }

        // Randomised 3-D cases.
        for _ in 0..80 {
            let v: Vec<i64> = (0..9).map(|_| rng.range(-3, 3)).collect();
            let basis = mat(3, 3, &v);
            if basis.determinant().unwrap().is_zero() {
                continue;
            }
            let lower: Vec<Rat> = (0..3).map(|_| rat(rng.range(-4, 4))).collect();
            let upper: Vec<Rat> = (0..3)
                .map(|i| &lower[i] + rat(rng.range(0, 5)))
                .collect();
            let offset: Vec<Rat> = (0..3).map(|_| rat(rng.range(-2, 2))).collect();
            run(
                &basis,
                &offset,
                &lower,
                &upper,
                &mut cases,
                &mut skipped,
                &mut points_found,
            );
        }

        assert!(
            cases >= 250,
            "only {cases} cross-checked cases ({skipped} skipped as too large)"
        );
        assert!(
            points_found >= 2000,
            "cross-check found only {points_found} points; it may be vacuous"
        );
    }

    #[test]
    fn integer_lattice_gives_the_integer_points_of_the_box() {
        for n in 1..=3usize {
            let basis = RatMatrix::identity(n);
            let lower = vec![rat(-2); n];
            let upper = vec![rat(3); n];
            let offset = vec![Rat::zero(); n];
            let got = enumerate_in_box(&basis, &offset, &lower, &upper).unwrap();
            assert_eq!(got.len(), 6usize.pow(n as u32));

            let set: BTreeSet<Vec<Rat>> = got.into_iter().collect();
            // Spot-check the corners and a fractional non-member.
            assert!(set.contains(&vec![rat(-2); n]));
            assert!(set.contains(&vec![rat(3); n]));
            assert!(!set.contains(&vec![rat(4); n]));
        }

        // Non-integer box edges: only the integers strictly inside count.
        let got = enumerate_in_box(
            &RatMatrix::identity(1),
            &[Rat::zero()],
            &[ratio(-3, 2)],
            &[ratio(5, 2)],
        )
        .unwrap();
        assert_eq!(got, vec![rats(&[-1]), rats(&[0]), rats(&[1]), rats(&[2])]);
    }

    #[test]
    fn empty_boxes_return_nothing_and_terminate() {
        let basis = mat(2, 2, &[1, 0, 0, 1]);
        // Inverted bounds on one axis.
        let got = enumerate_in_box(&basis, &rats(&[0, 0]), &rats(&[5, 0]), &rats(&[-5, 10]))
            .unwrap();
        assert!(got.is_empty());

        // A well-formed but empty box: sits strictly between lattice points.
        let coarse = mat(2, 2, &[10, 0, 0, 10]);
        let got = enumerate_in_box(
            &coarse,
            &rats(&[0, 0]),
            &[ratio(1, 2), ratio(1, 2)],
            &[ratio(9, 2), ratio(9, 2)],
        )
        .unwrap();
        assert!(got.is_empty());

        // A degenerate zero-volume box that does contain a point.
        let got = enumerate_in_box(&coarse, &rats(&[0, 0]), &rats(&[10, 0]), &rats(&[10, 0]))
            .unwrap();
        assert_eq!(got, vec![rats(&[10, 0])]);
    }

    #[test]
    fn one_dimensional_lattice_gives_the_multiples() {
        for m in [1i64, 2, 3, 7, 12] {
            let basis = mat(1, 1, &[m]);
            let (lo, hi) = (-20i64, 33i64);
            let got = enumerate_in_box(&basis, &[Rat::zero()], &[rat(lo)], &[rat(hi)]).unwrap();
            let want: Vec<Vec<Rat>> = (lo..=hi)
                .filter(|v| v.rem_euclid(m) == 0)
                .map(|v| rats(&[v]))
                .collect();
            assert_eq!(got, want, "multiples of {m} in [{lo}, {hi}]");
        }
    }

    #[test]
    fn negative_and_fractional_generators_still_work() {
        // A lattice generated by a negative step is the same lattice.
        let got = enumerate_in_box(&mat(1, 1, &[-5]), &[Rat::zero()], &[rat(-9)], &[rat(11)])
            .unwrap();
        assert_eq!(
            got,
            vec![rats(&[-5]), rats(&[0]), rats(&[5]), rats(&[10])]
        );

        // Half-integer generator: the lattice is (1/2)Z.
        let basis = RatMatrix::from_rows(&[vec![ratio(1, 2)]]);
        let got = enumerate_in_box(&basis, &[Rat::zero()], &[Rat::zero()], &[rat(2)]).unwrap();
        assert_eq!(
            got,
            vec![
                rats(&[0]),
                vec![ratio(1, 2)],
                rats(&[1]),
                vec![ratio(3, 2)],
                rats(&[2])
            ]
        );
    }

    #[test]
    fn offset_shifts_the_box_not_the_lattice() {
        let basis = RatMatrix::identity(1);
        // x + 1/2 in [0, 2]  <=>  x in [-1/2, 3/2]  =>  {0, 1}.
        // With the sign flipped it would be {1, 2}, so this pins the direction.
        let got = enumerate_in_box(&basis, &[ratio(1, 2)], &[rat(0)], &[rat(2)]).unwrap();
        assert_eq!(got, vec![rats(&[0]), rats(&[1])]);

        // Offsetting by o is the same as shifting the box by -o.
        let lattice = mat(2, 2, &[3, 1, 1, 4]);
        let offset = rats(&[2, -5]);
        let lower = rats(&[-4, -1]);
        let upper = rats(&[6, 9]);
        let with_offset = enumerate_in_box(&lattice, &offset, &lower, &upper).unwrap();
        let shifted_lo: Vec<Rat> = (0..2).map(|i| &lower[i] - &offset[i]).collect();
        let shifted_hi: Vec<Rat> = (0..2).map(|i| &upper[i] - &offset[i]).collect();
        let without =
            enumerate_in_box(&lattice, &rats(&[0, 0]), &shifted_lo, &shifted_hi).unwrap();
        assert_eq!(with_offset, without);
        assert!(!with_offset.is_empty());

        // Translating the whole problem translates every answer.
        let delta = rats(&[3, 1]);
        let moved_lo: Vec<Rat> = (0..2).map(|i| &lower[i] + &delta[i]).collect();
        let moved_hi: Vec<Rat> = (0..2).map(|i| &upper[i] + &delta[i]).collect();
        let moved_off: Vec<Rat> = (0..2).map(|i| &offset[i] + &delta[i]).collect();
        let moved = enumerate_in_box(&lattice, &moved_off, &moved_lo, &moved_hi).unwrap();
        assert_eq!(moved, with_offset);
    }

    #[test]
    fn enumeration_handles_a_lattice_of_lower_rank_than_the_ambient_space() {
        // A line through 3-space: multiples of (1, 2, 3).
        let basis = mat(1, 3, &[1, 2, 3]);
        let got = enumerate_in_box(
            &basis,
            &rats(&[0, 0, 0]),
            &rats(&[-2, -10, -10]),
            &rats(&[4, 10, 10]),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![rats(&[-2, -4, -6]), rats(&[-1, -2, -3]), rats(&[0, 0, 0]),
                 rats(&[1, 2, 3]), rats(&[2, 4, 6]), rats(&[3, 6, 9])]
        );

        // A box that misses the line entirely.
        let got = enumerate_in_box(
            &basis,
            &rats(&[0, 0, 0]),
            &rats(&[1, 1, 1]),
            &rats(&[1, 1, 1]),
        )
        .unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn dependent_generators_are_reduced_to_a_basis_before_enumerating() {
        // Three generators of a rank-2 lattice; the answer must not contain
        // duplicates even though many coefficient vectors give the same point.
        let basis = mat(3, 2, &[1, 0, 0, 1, 1, 1]);
        let got = enumerate_in_box(&basis, &rats(&[0, 0]), &rats(&[0, 0]), &rats(&[2, 2]))
            .unwrap();
        let unique: BTreeSet<Vec<Rat>> = got.iter().cloned().collect();
        assert_eq!(got.len(), unique.len(), "duplicate points emitted");
        assert_eq!(got.len(), 9);
    }

    #[test]
    fn dimension_mismatches_are_rejected() {
        let basis = mat(2, 2, &[1, 0, 0, 1]);
        assert!(enumerate_in_box(&basis, &rats(&[0]), &rats(&[0, 0]), &rats(&[1, 1])).is_err());
        assert!(enumerate_in_box(&basis, &rats(&[0, 0]), &rats(&[0]), &rats(&[1, 1])).is_err());
        assert!(enumerate_in_box(&basis, &rats(&[0, 0]), &rats(&[0, 0]), &rats(&[1])).is_err());
    }

    /// The shape every LCG-derived constraint eventually takes: the lattice
    /// `{(x, a*x mod m)}`, spanned by `(1, a)` and `(0, m)`, intersected with a
    /// box that is narrow in `x` and wide in `y`.
    ///
    /// Two things are being proved. First, that enumeration is still exactly
    /// complete when the numbers are realistic (`m = 2^48`, `a` the
    /// `java.util.Random` multiplier) rather than the single digits the
    /// brute-force cross-check can afford — here the oracle is a direct scan
    /// over `x`, which is possible because the box pins `x` to a short range.
    /// Second, that the per-axis scaling the docs insist on is not optional:
    /// unscaled, the circumscribing ball of this box has radius ~2^40 and
    /// contains on the order of 2^32 lattice points, so the enumeration is
    /// hopeless however correct it is.
    #[test]
    fn scaled_modular_lattice_matches_a_direct_scan() {
        let a: i128 = 25_214_903_917; // java.util.Random's multiplier
        let m: i128 = 1 << 48;
        let x_max: i128 = 5000;
        let d: i128 = 1 << 40;

        // Scale axis i by L / side_i with L = x_max * 2d, which turns the
        // 5000-by-2^41 box into a square.
        let sx: i128 = 2 * d;
        let sy: i128 = x_max;

        let basis = mat(
            2,
            2,
            &[sx as i64, (a * sy) as i64, 0, (m * sy) as i64],
        );
        let lower = rats(&[0, (-d * sy) as i64]);
        let upper = rats(&[(x_max * sx) as i64, (d * sy) as i64]);
        let got = enumerate_in_box(&basis, &rats(&[0, 0]), &lower, &upper).unwrap();

        // Direct scan: for each x, a*x mod m has exactly one representative
        // that could be within d of zero, since d < m/2.
        let mut want: Vec<Vec<Rat>> = Vec::new();
        for x in 0..=x_max {
            let r = (a * x).rem_euclid(m);
            let y = if r <= d {
                r
            } else if m - r <= d {
                r - m
            } else {
                continue;
            };
            want.push(rats(&[(x * sx) as i64, (y * sy) as i64]));
        }
        want.sort();

        assert_eq!(got, want);
        assert!(
            want.len() >= 10,
            "only {} hits; the test would be near-vacuous",
            want.len()
        );

        // And the same problem unscaled, to show the advice in the docs is
        // load-bearing rather than decorative.
        // A small cap keeps the test quick: the very first level of the
        // unscaled search spans tens of thousands of candidates on its own, so
        // the budget check rejects it without ever descending.
        let raw = mat(2, 2, &[1, a as i64, 0, m as i64]);
        let blown = enumerate_in_box_capped(
            &raw,
            &rats(&[0, 0]),
            &rats(&[0, -(d as i64)]),
            &rats(&[x_max as i64, d as i64]),
            500,
        );
        assert!(blown.is_err(), "the unscaled box should exhaust the budget");
    }

    #[test]
    fn the_node_cap_fails_instead_of_hanging() {
        let basis = RatMatrix::identity(2);
        let big = rat(1_000_000);
        let err = enumerate_in_box_capped(
            &basis,
            &rats(&[0, 0]),
            &[-big.clone(), -big.clone()],
            &[big.clone(), big],
            1000,
        );
        assert!(err.is_err(), "an enormous box should exhaust the budget");

        // The same box with a generous cap on a small region still succeeds.
        let ok = enumerate_in_box_capped(
            &basis,
            &rats(&[0, 0]),
            &rats(&[0, 0]),
            &rats(&[2, 2]),
            DEFAULT_MAX_NODES,
        )
        .unwrap();
        assert_eq!(ok.len(), 9);
    }

    #[test]
    fn sqrt_bounds_round_outward() {
        for (n, d) in [(0i64, 1i64), (1, 1), (2, 1), (9, 4), (7, 3), (1, 7), (10_000, 3)] {
            let q = ratio(n, d);
            let b = sqrt_upper_bound(&q);
            assert!(!b.is_negative());
            assert!(&b * &b >= q, "sqrt bound for {n}/{d} is too small");
        }
        // Exact squares should be tight, or the enumeration intervals bloat.
        assert_eq!(sqrt_upper_bound(&ratio(9, 4)), ratio(3, 2));
        assert_eq!(sqrt_upper_bound(&rat(16)), rat(4));
    }
}
