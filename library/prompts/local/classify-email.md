---
title: Sort an email into a category (small local model)
use: email/classify
model: local
tags: email, classify, local
---
Classify the email below into exactly one of: load_offer, rate_confirmation, invoice_or_bill, payment, travel,
calendar, personal, newsletter, spam, other.
Answer with only the category word.

Subject: {{subject}}
From: {{from}}
{{text}}
