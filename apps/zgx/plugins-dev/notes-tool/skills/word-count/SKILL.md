---
name: word-count
description: Use when the person wants a text trimmed or checked against a word limit.
---

1. Call `notes_tool__count_words` with the text and report the words, lines and characters.
2. If the person gave a limit and the text is over it, propose a shorter version and call `notes_tool__count_words` on it before you show it.
3. Stop when the count is at or under the limit.
