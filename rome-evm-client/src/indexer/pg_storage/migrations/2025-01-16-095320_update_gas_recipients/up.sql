UPDATE eth_block
SET gas_recipient = CONCAT('0x', SUBSTRING(gas_recipient FROM 27))
WHERE gas_recipient IS NOT NULL AND LENGTH(gas_recipient) > 42;
