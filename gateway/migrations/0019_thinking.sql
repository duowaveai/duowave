-- Thinking's line of a request (docs/plans/thinking.md, «The turn's line», D2): how `OurThinking` answered the turn —
-- the settings' mark and the variants, the agent, the fast route's step, the card's parts and tokens, each round's
-- times, tokens and calls, the first word, the waits, the shadow's hits and misses, «Чутьё»'s answers, and what the
-- person did next. JSON, no words of the phrase (the row has them, for whoever may read the journal); pruned with the
-- journal. A request Thinking did not answer has none.
--
-- 0019 is this migration's for good (the owner, 27.11.2026); the work-systems branch, which had 0019 too, takes 0020.
-- A migration in main is never renumbered or edited: a database that applied it refuses a changed one.
ALTER TABLE requests ADD COLUMN thinking TEXT;
