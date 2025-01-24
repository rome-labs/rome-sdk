CREATE TYPE BlockParams AS (
    blockhash VARCHAR(66),
    parent_hash VARCHAR(66),
    number BIGINT,
    block_timestamp NUMERIC
);

------------------------------------------------------------------------------------------------------------------------

-- eth block headers
CREATE TABLE eth_block (
    slot_number BIGINT NOT NULL,
    slot_block_idx INTEGER NOT NULL,

    PRIMARY KEY (slot_number, slot_block_idx),
    CONSTRAINT fk_slot_number
        FOREIGN KEY (slot_number)
            REFERENCES sol_slot(slot_number),

    block_gas_used NUMERIC NOT NULL,
    gas_recipient VARCHAR(42) DEFAULT NULL,
    slot_timestamp BIGINT,

    -- Filled after block production
    params BlockParams DEFAULT NULL
);

CREATE INDEX eth_block_hash ON eth_block USING HASH (((params).blockhash));
CREATE INDEX eth_block_number ON eth_block(((params).number));

------------------------------------------------------------------------------------------------------------------------

-- Links transaction results to blocks
CREATE TABLE eth_block_txs (
    slot_number BIGINT NOT NULL,
    slot_block_idx INTEGER NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,

    PRIMARY KEY (slot_number, slot_block_idx, tx_hash),
    CONSTRAINT fk_slot_number_slot_block_idx
        FOREIGN KEY (slot_number, slot_block_idx)
            REFERENCES eth_block(slot_number, slot_block_idx),

    CONSTRAINT fk_slot_number_tx_hash
        FOREIGN KEY (slot_number, tx_hash)
            REFERENCES evm_tx_result(slot_number, tx_hash),

    tx_idx INTEGER NOT NULL
);

------------------------------------------------------------------------------------------------------------------------

CREATE INDEX sol_slot_parent ON sol_slot(parent_slot);

-- Returns not produced Ethereum blocks which exists on the longest chain of Solana blocks
CREATE RECURSIVE VIEW pending_blocks (slot_number, parent_slot, slot_block_idx, gas_recipient, slot_timestamp)
AS SELECT
    first_s.slot_number,
    first_s.parent_slot,
    first_eb.slot_block_idx,
    first_eb.gas_recipient,
    first_eb.slot_timestamp
FROM sol_slot AS first_s
         LEFT JOIN eth_block AS first_eb ON first_s.slot_number = first_eb.slot_number
WHERE first_s.slot_number = (SELECT MAX(slot_number) FROM eth_block) AND first_eb.params IS NULL
UNION
SELECT
    next_s.slot_number,
    next_s.parent_slot,
    next_eb.slot_block_idx,
    next_eb.gas_recipient,
    next_eb.slot_timestamp
FROM sol_slot AS next_s
         INNER JOIN pending_blocks p ON p.parent_slot = next_s.slot_number
         LEFT JOIN eth_block AS next_eb ON next_s.slot_number = next_eb.slot_number AND next_eb.params IS NULL
WHERE next_s.slot_number > 0
  AND NOT EXISTS(
    SELECT 1
    FROM eth_block b
    WHERE b.slot_number = next_s.slot_number AND b.params IS NOT NULL
);

------------------------------------------------------------------------------------------------------------------------

-- Returns all transactions contained inside a chain of not produced blocks
CREATE OR REPLACE VIEW pending_transactions AS
SELECT
    pb.slot_number,
    pb.parent_slot,
    pb.slot_block_idx,
    pb.gas_recipient,
    pb.slot_timestamp,
    ebt.tx_idx,
    et.rlp,
    etr.tx_result
FROM pending_blocks pb
         LEFT JOIN eth_block_txs AS ebt ON ebt.slot_number = pb.slot_number AND ebt.slot_block_idx = pb.slot_block_idx
         LEFT JOIN evm_tx_result AS etr ON etr.slot_number = ebt.slot_number AND etr.tx_hash = ebt.tx_hash
         LEFT JOIN evm_tx AS et ON et.tx_hash = ebt.tx_hash;


------------------------------------------------------------------------------------------------------------------------

-- Returns last produced blockhash of Ethereum block (if any) from a given Solana slot
CREATE FUNCTION get_last_produced_block(slot_number_val BIGINT)
    RETURNS VARCHAR(66)
AS $$
    DECLARE
        result VARCHAR(66) = NULL;
BEGIN
        SELECT ((params).blockhash) INTO result
        FROM eth_block
        WHERE eth_block.slot_number = slot_number_val
        ORDER BY slot_block_idx DESC LIMIT 1;
        RETURN result;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

-- Returns latest solana slot number which contains produced Ethereum block
CREATE FUNCTION get_max_slot_produced()
    RETURNS BIGINT
AS $$
DECLARE
    result BIGINT = NULL;
BEGIN
    SELECT MAX(slot_number) INTO result
    FROM eth_block
    WHERE eth_block.params IS NOT NULL;
    RETURN result;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

-- Updates params of a given produced Ethereum block
CREATE PROCEDURE block_produced(
    slot_number_val BIGINT,
    slot_block_idx_val INT,
    params_val TEXT
)  LANGUAGE plpgsql AS $$
BEGIN
    UPDATE public.eth_block
    SET params=params_val::blockparams
    WHERE slot_number=slot_number_val
      AND slot_block_idx=slot_block_idx_val
      AND params IS NULL;
END;
$$;

------------------------------------------------------------------------------------------------------------------------

-- Returns Solana slot number of a slot where given Ethereum block number exists
CREATE FUNCTION get_slot_for_eth_block(block_number_val BIGINT)
    RETURNS BIGINT
AS $$
DECLARE
    result BIGINT = NULL;
BEGIN
    SELECT slot_number INTO result
    FROM eth_block
    WHERE
        params IS NOT NULL
      AND ((params).number) = block_number_val;
    RETURN result;
END;
$$ LANGUAGE plpgsql;

------------------------------------------------------------------------------------------------------------------------

-- Returns set of already produced Ethereum blocks
-- between from_slot and to_slot Solana slots, including to_slot
CREATE OR REPLACE FUNCTION reproduce_blocks(from_slot BIGINT, to_slot BIGINT)
    RETURNS TABLE (
        slot_number BIGINT,
        slot_block_idx INTEGER,
        gas_recipient VARCHAR(42),
        slot_timestamp BIGINT,
        blockhash VARCHAR(66),
        parent_hash VARCHAR(66),
        block_number BIGINT,
        block_timestamp TEXT,
        tx_idx INTEGER,
        rlp BYTEA,
        tx_result JSONB
                  )
AS $$
BEGIN
    RETURN QUERY SELECT
                     eb.slot_number,
                     eb.slot_block_idx,
                     eb.gas_recipient,
                     eb.slot_timestamp,
                     (eb.params).blockhash,
                     (eb.params).parent_hash,
                     (eb.params).number,
                     (eb.params).block_timestamp::TEXT,
                     ebt.tx_idx,
                     et.rlp,
                     etr.tx_result
                 FROM eth_block eb
                          INNER JOIN eth_block_txs AS ebt ON ebt.slot_number = eb.slot_number AND ebt.slot_block_idx = eb.slot_block_idx
                          INNER JOIN evm_tx_result AS etr ON etr.slot_number = ebt.slot_number AND etr.tx_hash = ebt.tx_hash
                          INNER JOIN evm_tx AS et ON et.tx_hash = ebt.tx_hash
                 WHERE
                     eb.slot_number >= from_slot
                   AND eb.slot_number < to_slot
                   AND eb.params IS NOT NULL;
END;
$$ LANGUAGE plpgsql;
