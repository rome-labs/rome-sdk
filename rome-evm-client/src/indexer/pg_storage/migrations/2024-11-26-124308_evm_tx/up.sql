CREATE TABLE evm_tx (
    tx_hash VARCHAR(66) PRIMARY KEY,
    rlp BYTEA DEFAULT NULL
);

CREATE TABLE evm_tx_result (
    slot_number BIGINT NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,

    PRIMARY KEY (slot_number, tx_hash),
    CONSTRAINT fk_slot_number
        FOREIGN KEY (slot_number)
            REFERENCES sol_slot(slot_number),

    CONSTRAINT fk_tx_hash
        FOREIGN KEY (tx_hash)
            REFERENCES evm_tx(tx_hash),

    tx_result JSONB NOT NULL,
    receipt_params JSONB DEFAULT NULL
);
