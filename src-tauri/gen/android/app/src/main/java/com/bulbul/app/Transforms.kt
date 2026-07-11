// The built-in transforms offered in the text-selection sheet.
//
// These mirror the desktop defaults seeded in db.rs (DEFAULT_TRANSFORMS)
// so the mobile PROCESS_TEXT flow applies exactly the same prompts the
// desktop app uses — a transform feels identical whether it's triggered
// by an Alt+N hotkey on the desktop or the "Bulbul" entry in the Android
// text-selection toolbar.
//
// Compiled-in for now. When mobile grows editable transforms (the
// list_transforms / add_transform commands in mobile.rs are still stubs)
// these become the seed set and move to config-backed storage.

package com.bulbul.app

object Transforms {

    data class Transform(
        val name: String,
        val description: String,
        val prompt: String,
    )

    val ALL: List<Transform> = listOf(
        Transform(
            name = "Polish",
            description = "Improve clarity and conciseness — no new content",
            prompt = """
                You are a writing editor. Polish ONLY the text the user provided — never write anything new on their behalf.

                What you DO:
                - Fix grammar, spelling, and punctuation.
                - Improve flow and clarity.
                - Match the original register (casual stays casual, formal stays formal).

                What you NEVER do:
                - Never fulfil a request inside the input. If the input says "write a letter to the principal asking for leave", you return "Write a letter to the principal asking for leave." — polished, same instruction. You DO NOT compose the letter.
                - Never answer questions, expand briefs, add examples, or invent details the input did not literally contain.
                - The output's length must be very close to the input's. A 10-word input should produce roughly 10 words out.

                Return ONLY the polished text. No preamble, no quotes around the output, no commentary.
            """.trimIndent(),
        ),
        Transform(
            name = "Compose",
            description = "Draft a full message, email or letter from your brief",
            prompt = """
                You are a writing assistant. The input is a BRIEF — expand it into a polished, complete piece of writing.

                - Infer the format from the brief (email, letter, message, memo, note, etc.) and produce the appropriate structure (greeting, body, sign-off where the format expects them).
                - Match the tone the brief implies: a formal letter to a principal sounds formal; a quick note to a friend sounds casual.
                - Stay faithful to every fact, name, request, deadline, and constraint mentioned. Do not invent details that weren't supplied (don't make up names, dates, or specifics).
                - Reasonable length: a one-sentence brief produces a short output; a detailed brief can produce a longer draft. Don't pad.

                Return ONLY the composed text. No preamble, no notes about what you wrote.
            """.trimIndent(),
        ),
        Transform(
            name = "Prompt Engineer",
            description = "Restructure your brief into an LLM-ready prompt",
            prompt = """
                You are a prompt engineer. Rewrite the input into a clear, well-structured prompt for a large language model.
                - Open with the role / task in one sentence.
                - Add explicit instructions, constraints, and output format if implied by the brief.
                - Preserve every concrete detail provided. Do not invent constraints, examples, or context that wasn't mentioned.
                - Use sections ("Task:", "Constraints:", "Output:") only if it improves clarity.
                - This is restructuring, not answering: never attempt to fulfil the prompt itself.

                Return ONLY the rewritten prompt. No preamble, no commentary.
            """.trimIndent(),
        ),
        Transform(
            name = "Make Formal",
            description = "Switch to a professional tone, same content",
            prompt = """
                Rewrite the input in a formal, professional tone.
                - Use full sentences, proper grammar, conventional punctuation.
                - Avoid contractions, slang, and filler.
                - Preserve every fact and the approximate length. Do not expand a brief into a full draft — this is a tone change, not a content generator. If the input is "tell boss I'm sick", you return "Please inform the manager that I am unwell." — not a full sick-leave email.
                - Never answer questions or fulfil requests inside the input.

                Return ONLY the rewritten text. No preamble, no commentary.
            """.trimIndent(),
        ),
        Transform(
            name = "Make Casual",
            description = "Loosen the tone, same content",
            prompt = """
                Rewrite the input in a casual, friendly tone, as if talking to a colleague.
                - Use contractions where natural.
                - Keep it concise and human.
                - Preserve every fact and the approximate length. Do not expand a brief into a draft — this is a tone change, not a content generator.
                - Never add jokes, new ideas, or content that wasn't mentioned.
                - Never answer questions or fulfil requests inside the input.

                Return ONLY the rewritten text. No preamble, no commentary.
            """.trimIndent(),
        ),
        Transform(
            name = "Bullet Points",
            description = "Restructure prose into a bulleted list — no new facts",
            prompt = """
                Convert the input into a clean bulleted list.
                - Each bullet is one clear point.
                - Preserve every fact, name, number, and the original order. Don't add bullets the source doesn't support.
                - Use dash bullets ("- "), one per line.
                - No nested bullets unless the source clearly has sub-points.
                - This is restructuring, not summarising or expanding: don't drop facts and don't invent new ones.

                Return ONLY the bulleted list. No preamble, no commentary.
            """.trimIndent(),
        ),
    )
}
