/-
  Supply conservation for transfers: a model of the account-balance transition
  (`StateDB` transfer: signature → nonce → balance → apply, checked arithmetic).

  Balances are a function from account index to amount over accounts `[0, n)`.
  A transfer that passes its balance check moves value and creates none; one
  that fails changes nothing. Hand-written model (not Aeneas); the property is
  the brief's "no tokens from nothing".
-/

namespace Maya2C.Supply

/-- Sum of balances of accounts `0 .. n`. -/
def total : Nat → (Nat → Nat) → Nat
  | 0, _ => 0
  | n + 1, f => total n f + f n

/-- `f` with account `k` set to `v`. -/
def update (f : Nat → Nat) (k v : Nat) : Nat → Nat :=
  fun x => if x = k then v else f x

theorem total_update_out (n : Nat) (f : Nat → Nat) (k v : Nat) (hk : n ≤ k) :
    total n (update f k v) = total n f := by
  induction n with
  | zero => rfl
  | succ n ih =>
    have hne : n ≠ k := by omega
    have hn : update f k v n = f n := by simp [update, hne]
    show total n (update f k v) + update f k v n = total n f + f n
    rw [hn, ih (by omega)]

theorem total_update (n : Nat) (f : Nat → Nat) (k v : Nat) (hk : k < n) :
    total n (update f k v) + f k = total n f + v := by
  induction n with
  | zero => omega
  | succ n ih =>
    show total n (update f k v) + update f k v n + f k = total n f + f n + v
    by_cases h : k = n
    · subst h
      have hout := total_update_out k f k v (Nat.le_refl k)
      have hv : update f k v k = v := by simp [update]
      rw [hv, hout]
      omega
    · have hne : n ≠ k := fun e => h e.symm
      have hn : update f k v n = f n := by simp [update, hne]
      have := ih (by omega)
      rw [hn]
      omega

/-- Move `amt` from `i` to `j`, if `i` holds it; otherwise nothing changes. -/
def transfer (f : Nat → Nat) (i j amt : Nat) : Nat → Nat :=
  if amt ≤ f i then
    let g := update f i (f i - amt)
    update g j (g j + amt)
  else f

/-- No tokens from nothing: every transfer between two distinct accounts
    preserves the total, whether it succeeds or is refused. -/
theorem transfer_conserves (n : Nat) (f : Nat → Nat) (i j amt : Nat)
    (hi : i < n) (hj : j < n) (hij : i ≠ j) :
    total n (transfer f i j amt) = total n f := by
  unfold transfer
  split
  · rename_i hle
    have h1 := total_update n f i (f i - amt) hi
    have h2 := total_update n (update f i (f i - amt)) j (update f i (f i - amt) j + amt) hj
    have hgj : update f i (f i - amt) j = f j := by
      simp only [update]; exact if_neg (fun e => hij e.symm)
    rw [hgj] at h2
    show total n (update (update f i (f i - amt)) j (update f i (f i - amt) j + amt)) = total n f
    rw [hgj]
    omega
  · rfl

/-- A refused transfer (insufficient balance) is a no-op. -/
theorem refused_transfer_is_noop (f : Nat → Nat) (i j amt : Nat) (h : f i < amt) :
    transfer f i j amt = f := by
  unfold transfer
  simp [Nat.not_le.mpr h]

end Maya2C.Supply
