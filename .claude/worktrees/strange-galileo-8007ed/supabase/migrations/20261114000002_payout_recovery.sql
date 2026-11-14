-- Payouts that will never pay no longer block their pay period, and the
-- signer keeps track of the nonces it has used.

-- Set when a payout is given up for good and its cap reservation released:
-- the final failed send, or an authorization that expired before anything
-- was sent. Payroll may then create a new payout for the same period.
ALTER TABLE axom.payouts ADD COLUMN abandoned_at TIMESTAMPTZ;

-- Payouts already in that state: failed, no retry scheduled, reservation released.
UPDATE axom.payouts p SET abandoned_at = now()
 WHERE p.status = 'failed' AND p.next_attempt_at IS NULL
   AND EXISTS (SELECT 1 FROM axom.cap_ledger l WHERE l.payout_id = p.id AND l.kind = 'release');

-- The nonce of the transaction signed for this payout. NEAR reports an
-- access key's nonce a couple of blocks behind, so the next payout takes
-- the higher of the chain's nonce and the last one used here.
ALTER TABLE axom.payouts ADD COLUMN tx_nonce BIGINT;

-- One live payout per worker per pay period. A payout that never paid and
-- never will (blocked by a gate, over the cap, approval expired, abandoned)
-- does not count, so the period can be paid once the cause is gone. A
-- rejected payout does count: an admin decided not to pay it.
DROP INDEX axom.payouts_worker_period_idx;
CREATE UNIQUE INDEX payouts_worker_period_idx ON axom.payouts (worker_id, period_start)
    WHERE worker_id IS NOT NULL AND period_start IS NOT NULL
      AND status NOT IN ('blocked', 'cap_exceeded', 'expired') AND abandoned_at IS NULL;
