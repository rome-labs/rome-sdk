------------------------------------------------------------------------------------------------------------------------
-- Improved version of the function.
-- Only updates slots residing on a same fork

CREATE OR REPLACE FUNCTION set_finalized_slot(slot BIGINT)
    RETURNS TABLE (
                      slot_number BIGINT,
                      block_data BYTEA
                  )
    LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
        WITH RECURSIVE fork AS
            -- Find all not-finalized slots on a fork ending with a given slot
            (
            SELECT
                first_s.slot_number,
                first_s.parent_slot
            FROM sol_slot AS first_s
            WHERE first_s.slot_number = slot
              AND first_s.status <> 'Finalized'::slotstatus
            UNION
            SELECT
                parent.slot_number,
                parent.parent_slot
            FROM sol_slot AS parent
                     INNER JOIN fork AS f
                                ON f.parent_slot = parent.slot_number
            WHERE parent.status <> 'Finalized'::slotstatus
            )
            -- Set slot statuses to finalized, return block data for found slots
            UPDATE sol_slot AS s SET status = 'Finalized'::slotstatus
                FROM fork AS f, sol_block AS b
                WHERE s.slot_number = f.slot_number AND s.slot_number = b.slot_number
                RETURNING s.slot_number, b.block_data;
END;
$$;

------------------------------------------------------------------------------------------------------------------------

-- Returns Ethereum blocks which resides on the longest chain of Solana blocks
CREATE OR REPLACE RECURSIVE VIEW pending_blocks_with_slot_statuses2 (
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
            INNER JOIN pending_blocks_with_slot_statuses2 p ON p.parent_slot = parent.slot_number
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
