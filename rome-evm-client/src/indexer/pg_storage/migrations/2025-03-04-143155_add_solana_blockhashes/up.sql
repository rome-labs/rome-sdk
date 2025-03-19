ALTER TABLE sol_slot
    ADD COLUMN blockhash TEXT,
    ADD COLUMN timestamp BIGINT;

------------------------------------------------------------------------------------------------------------------------

CREATE PROCEDURE set_block_with_status2(
    slot_number_val BIGINT,
    parent_slot_val BIGINT,
    block BYTEA,
    status_val slotstatus,
    blockhash_val TEXT,
    timestamp_val BIGINT
)  LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO sol_slot (slot_number, parent_slot, status, blockhash, timestamp)
    VALUES (
            slot_number_val,
            parent_slot_val,
            status_val,
            blockhash_val,
            timestamp_val
           )
    ON CONFLICT DO NOTHING;

    INSERT INTO sol_block (slot_number, block_data)
    VALUES (slot_number_val, block)
    ON CONFLICT DO NOTHING;
END;
$$;

------------------------------------------------------------------------------------------------------------------------

CREATE PROCEDURE update_finalized_block2(
    slot_number_val BIGINT,
    parent_slot_val BIGINT,
    block BYTEA,
    blockhash_val TEXT,
    timestamp_val BIGINT
)  LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO sol_slot (slot_number, parent_slot, status, blockhash, timestamp)
    VALUES (
            slot_number_val,
            parent_slot_val,
            'Finalized'::slotstatus,
            blockhash_val,
            timestamp_val
           )
    ON CONFLICT DO UPDATE SET
                              parent_slot = excluded.parent_slot,
                              status = excluded.status,
                              blockhash = excluded.blockhash,
                              timestamp = excluded.timestamp;

    INSERT INTO sol_block (slot_number, block_data)
    VALUES (slot_number_val, block)
    ON CONFLICT DO UPDATE SET block_data = excluded.block_data;
END;
$$;

------------------------------------------------------------------------------------------------------------------------

-- Returns Ethereum blocks which resides on the longest chain of Solana blocks
CREATE RECURSIVE VIEW pending_blocks_with_slot_statuses2 (
    slot_number, parent_slot, slot_status, slot_blockhash, slot_timestamp, slot_block_idx, gas_recipient, params
    )
AS SELECT
       first_s.slot_number,
       first_s.parent_slot,
       first_s.status,
       first_s.blockhash,
       first_s.timestamp,
       first_eb.slot_block_idx,
       first_eb.gas_recipient,
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
       parent.blockhash,
       parent.timestamp,
       next_eb.slot_block_idx,
       next_eb.gas_recipient,
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
CREATE OR REPLACE VIEW pending_transactions_with_slot_statuses2 (
    slot_number, slot_status, slot_blockhash, slot_timestamp, slot_block_idx, gas_recipient, eth_blockhash, tx_idx, rlp, tx_result
    )
AS SELECT
       pb.slot_number,
       pb.slot_status,
       pb.slot_blockhash,
       pb.slot_timestamp,
       pb.slot_block_idx,
       pb.gas_recipient,
       (pb.params).blockhash,
       ebt.tx_idx,
       et.rlp,
       etr.tx_result
   FROM pending_blocks_with_slot_statuses2 pb
            INNER JOIN eth_block_txs AS ebt ON ebt.slot_number = pb.slot_number AND ebt.slot_block_idx = pb.slot_block_idx
            INNER JOIN evm_tx AS et ON et.tx_hash = ebt.tx_hash
            INNER JOIN evm_tx_result AS etr ON etr.slot_number = ebt.slot_number AND etr.tx_hash = ebt.tx_hash;

------------------------------------------------------------------------------------------------------------------------

-- Returns set of already produced Ethereum blocks
-- between from_slot and to_slot Solana slots, including from_slot
CREATE OR REPLACE FUNCTION reproduce_blocks2(from_slot BIGINT, to_slot BIGINT)
    RETURNS TABLE (
                      slot_number BIGINT,
                      slot_timestamp BIGINT,
                      slot_block_idx INTEGER,
                      gas_recipient VARCHAR(42),
                      blockhash VARCHAR(66),
                      parent_hash VARCHAR(66),
                      block_number BIGINT,
                      tx_idx INTEGER,
                      rlp BYTEA,
                      tx_result JSONB
                  )
AS $$
BEGIN
    RETURN QUERY SELECT
                     eb.slot_number,
                     ss.timestamp,
                     eb.slot_block_idx,
                     eb.gas_recipient,
                     (eb.params).blockhash,
                     (eb.params).parent_hash,
                     (eb.params).number,
                     ebt.tx_idx,
                     et.rlp,
                     etr.tx_result
                 FROM eth_block eb
                          INNER JOIN public.sol_slot ss on ss.slot_number = eb.slot_number
                          INNER JOIN eth_block_txs AS ebt ON ebt.slot_number = eb.slot_number AND ebt.slot_block_idx = eb.slot_block_idx
                          INNER JOIN evm_tx_result AS etr ON etr.slot_number = ebt.slot_number AND etr.tx_hash = ebt.tx_hash
                          INNER JOIN evm_tx AS et ON et.tx_hash = ebt.tx_hash
                 WHERE
                     eb.slot_number >= from_slot
                   AND eb.slot_number < to_slot
                   AND eb.params IS NOT NULL;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION get_block_header2(block_number_value BIGINT)
    RETURNS TABLE (
                      block_gas_used TEXT,
                      gas_recipient VARCHAR(42),
                      slot_timestamp BIGINT,
                      blockhash VARCHAR(66),
                      parent_hash VARCHAR(66),
                      number BIGINT
                  )
AS $$
BEGIN
    RETURN QUERY
        SELECT
            eb.block_gas_used::TEXT,
            eb.gas_recipient,
            eb.slot_timestamp,
            (eb.params).blockhash,
            (eb.params).parent_hash,
            (eb.params).number
        FROM eth_block eb
        WHERE (eb.params).number = block_number_value;
END;
$$ LANGUAGE plpgsql;
