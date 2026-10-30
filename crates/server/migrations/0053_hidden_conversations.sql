create table hidden_conversations (
    account_id uuid not null references accounts(id),
    conversation_id uuid not null references conversations(id),
    primary key(account_id, conversation_id)
);
create index hidden_conversations_conversation on hidden_conversations(conversation_id, account_id);
