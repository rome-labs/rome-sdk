CREATE PROCEDURE set_block_with_status(
    slot_number_val BIGINT,
    parent_slot_val BIGINT,
    block BYTEA,
    status_val slotstatus
)  LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO sol_slot (slot_number, parent_slot, status)
    VALUES (slot_number_val, parent_slot_val, status_val)
    ON CONFLICT DO NOTHING;

    INSERT INTO sol_block (slot_number, block_data)
    VALUES (slot_number_val, block)
    ON CONFLICT DO NOTHING;
END;
$$;

CREATE PROCEDURE update_finalized_block(
    slot_number_val BIGINT,
    parent_slot_val BIGINT,
    block BYTEA
)  LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO sol_slot (slot_number, parent_slot, status)
    VALUES (slot_number_val, parent_slot_val, 'Finalized'::slotstatus)
    ON CONFLICT DO UPDATE SET parent_slot = excluded.parent_slot, status = excluded.status;

    INSERT INTO sol_block (slot_number, block_data)
    VALUES (slot_number_val, block)
    ON CONFLICT DO UPDATE SET block_data = excluded.block_data;
END;
$$;

CREATE OR REPLACE FUNCTION set_finalized_slot(slot BIGINT)
    RETURNS TABLE (
        slot_number BIGINT,
        block_data BYTEA
                  )
    LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
        UPDATE sol_slot s
            SET status = 'Finalized'::slotstatus
            FROM sol_block b
            WHERE
                s.status <> 'Finalized'::slotstatus
                    AND s.slot_number <= slot
                    AND s.slot_number = b.slot_number
            RETURNING s.slot_number, b.block_data;
END;
$$;

-- Returns Ethereum blocks which resides on the longest chain of Solana blocks
CREATE RECURSIVE VIEW pending_blocks_with_slot_statuses (
    slot_number, parent_slot, slot_status, slot_block_idx, gas_recipient, slot_timestamp, params
    )
AS SELECT
       first_s.slot_number,
       first_s.parent_slot,
       first_s.status,
       first_eb.slot_block_idx,
       first_eb.gas_recipient,
       first_eb.slot_timestamp,
       first_eb.params

   -- Starting from the highest solana block containing not produced ethereum blocks
   FROM sol_slot AS first_s
            LEFT JOIN eth_block AS first_eb
                      ON first_s.slot_number = first_eb.slot_number
                          AND first_eb.params IS NULL
   WHERE first_s.slot_number = (SELECT MAX(slot_number) FROM eth_block)
   UNION
   SELECT
       parent.slot_number,
       parent.parent_slot,
       parent.status,
       next_eb.slot_block_idx,
       next_eb.gas_recipient,
       next_eb.slot_timestamp,
       next_eb.params

   -- Traversing down through parent slots
   FROM sol_slot AS parent
            INNER JOIN pending_blocks_with_slot_statuses p ON p.parent_slot = parent.slot_number
            LEFT JOIN eth_block AS next_eb ON parent.slot_number = next_eb.slot_number

   -- Until zero slot found
   WHERE
       parent.slot_number > 0

     -- Or unless solana slot is finalized and does not contain not produced ethereum blocks.
     -- This latest finalized and produced block will also be included in results.
     AND (
       p.slot_status <> 'Finalized'::slotstatus
           OR NOT EXISTS(SELECT 1 FROM eth_block b WHERE b.slot_number = p.slot_number AND b.params IS NOT NULL)
       );

------------------------------------------------------------------------------------------------------------------------

-- Returns all transactions contained inside a chain of not produced blocks
CREATE OR REPLACE VIEW pending_transactions_with_slot_statuses (
        slot_number, slot_status, slot_block_idx, gas_recipient, slot_timestamp, blockhash, tx_idx, rlp, tx_result
    )
AS SELECT
    pb.slot_number,
    pb.slot_status,
    pb.slot_block_idx,
    pb.gas_recipient,
    pb.slot_timestamp,
    (pb.params).blockhash,
    ebt.tx_idx,
    et.rlp,
    etr.tx_result
FROM pending_blocks_with_slot_statuses pb
         INNER JOIN eth_block_txs AS ebt ON ebt.slot_number = pb.slot_number AND ebt.slot_block_idx = pb.slot_block_idx
         INNER JOIN evm_tx AS et ON et.tx_hash = ebt.tx_hash
         INNER JOIN evm_tx_result AS etr ON etr.slot_number = ebt.slot_number AND etr.tx_hash = ebt.tx_hash;

------------------------------------------------------------------------------------------------------------------------

-- Removes all produced eth-blocks and subsequent transactions starting from solana slot from_slot
-- Used for recovery and to handle reorg events
CREATE OR REPLACE PROCEDURE clean_from_slot(
    from_slot BIGINT
)  LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM eth_block_txs WHERE slot_number >= from_slot;
    DELETE FROM eth_block WHERE slot_number >= from_slot;
    DELETE FROM evm_tx_result WHERE slot_number >= from_slot;
END;
$$;