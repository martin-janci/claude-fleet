# Reply Actions (fleet-mobile) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the same five reply actions — Copy, Quote, Retry, Fork here, Rewind here — under every reply on the phone, against the `rewind_conversation` tool the hub already serves.

**Architecture:** The phone gets no engine of its own. It decodes one new optional field (`ConvTurn.prompt_uuid`), reimplements the pure "which buttons, which anchor" rule in Kotlin against the same cases the desktop tests, and calls one hub tool. Destructive actions are gated twice: on the hub version, and on the client token being `full`.

**Tech Stack:** Kotlin Multiplatform, Compose Multiplatform, kotlinx.serialization, Gradle.

**Spec:** `docs/superpowers/specs/2026-09-26-reply-actions-design.md` (in the `claude-fleet` repo)

**Prerequisite:** the `claude-fleet` plan (`docs/superpowers/plans/2026-09-26-reply-actions-fleet.md`) is merged **and released**, because the hub-version gate in Task 2 needs a real released version number to point at.

## Global Constraints

- **Repo:** `fleet-mobile`, a sibling of `claude-fleet`. Its main checkout is **shared with live sessions**, so cut this branch in a git worktree, never in the main checkout.
- **`prompt_uuid` is `String? = null`.** A hub that predates it sends nothing and the model must still decode — mirror the tolerance `ConvItemTest` already proves for `ConvItem`.
- **Rewind confirmation copy, verbatim:** "The conversation is rewound to before this turn. Your files are left as they are."
- **Destructive actions need a `full` token.** `send_prompt` is not in the hub's readonly allow-list; follow the same gate (`App.kt:550`).
- **Additive wire changes gate on the hub's version string, not the contract revision** — `semverAtLeast` + a `HUB_VERSION_*` constant, exactly as `HUB_VERSION_KEYS` does (`HubContract.kt:87`).
- **Verify commands:** `./gradlew :shared:jvmTest` for the shared tests, `./gradlew build` for everything. Check `scripts/` and the repo README for the canonical CI command and prefer it.

---

### Task 1: Decode the anchor

**Files:**
- Modify: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/model/Conversation.kt:198-216` (`data class ConvTurn`)
- Test: `shared/src/commonTest/kotlin/dev/claudefleet/mobile/model/ConvItemTest.kt`

**Interfaces:**
- Consumes: nothing.
- Produces: `ConvTurn.promptUuid: String?` (wire name `prompt_uuid`). Task 3 reads it.

- [ ] **Step 1: Write the failing tests**

Add to `shared/src/commonTest/kotlin/dev/claudefleet/mobile/model/ConvItemTest.kt`:

```kotlin
@Test
fun aTurnCarriesItsPromptUuid() {
    val wire = """{"prompt":"hi","items":[],"prompt_uuid":"aaaaaaaa-0000-0000-0000-000000000001"}"""
    val turn = Json { ignoreUnknownKeys = true }
        .decodeFromString(ConvTurn.serializer(), wire)
    assertEquals("aaaaaaaa-0000-0000-0000-000000000001", turn.promptUuid)
}

@Test
fun aTurnFromAHubWithoutTheFieldStillDecodes() {
    // The field is optional precisely so an older hub keeps working: the phone
    // then shows Copy and Quote and hides the three that need an anchor.
    val wire = """{"prompt":"hi","items":[]}"""
    val turn = Json { ignoreUnknownKeys = true }
        .decodeFromString(ConvTurn.serializer(), wire)
    assertNull(turn.promptUuid)
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
./gradlew :shared:jvmTest --tests '*ConvItemTest*'
```

Expected: FAIL to compile — `unresolved reference: promptUuid`.

- [ ] **Step 3: Add the field**

In `shared/src/commonMain/kotlin/dev/claudefleet/mobile/model/Conversation.kt`, as the last parameter of `data class ConvTurn`:

```kotlin
    /**
     * The JSONL uuid of the entry that opened this turn — the anchor every
     * truncation is expressed against ("keep strictly before this prompt").
     *
     * Null for a turn no prompt opened (a compact boundary, a
     * notification-only turn) and for a hub that predates the field. Both
     * cases mean the same thing here: no Rewind, no Retry. Fork still works,
     * because it anchors on a LATER turn.
     */
    @SerialName("prompt_uuid") val promptUuid: String? = null,
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
./gradlew :shared:jvmTest --tests '*ConvItemTest*'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add shared/src/commonMain/kotlin/dev/claudefleet/mobile/model/Conversation.kt shared/src/commonTest/kotlin/dev/claudefleet/mobile/model/ConvItemTest.kt
git commit -m "feat(model): decode each turn's prompt uuid

The truncation anchor for the reply actions. Optional, so a hub that
predates it still decodes and the phone falls back to Copy and Quote.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 2: The hub call and its version gate

**Files:**
- Modify: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubContract.kt` (add `HUB_VERSION_REWIND` beside `HUB_VERSION_KEYS` at `:87`)
- Modify: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubClient.kt` (add `rewindConversation` beside `sendPrompt` at `:293`)
- Modify: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubClient.kt:763` (`LIFECYCLE_TOOLS`)
- Test: `shared/src/commonTest/kotlin/dev/claudefleet/mobile/net/HubClientTest.kt`, `.../HubContractTest.kt`

**Interfaces:**
- Consumes: `ConvTurn.promptUuid` (Task 1); `semverAtLeast(version, floor)`.
- Produces:
  ```kotlin
  const val HUB_VERSION_REWIND: String = "<the release that ships rewind_conversation>"
  suspend fun HubClient.rewindConversation(
      sessionId: Long, mode: String, anchorUuid: String?, newWorktree: String? = null,
  ): SessionRow
  ```
  Task 3 calls both.

- [ ] **Step 1: Write the failing tests**

Add to `shared/src/commonTest/kotlin/dev/claudefleet/mobile/net/HubClientTest.kt`, following the file's existing fake-transport style:

```kotlin
@Test
fun rewindConversationSendsSnakeCaseArgs() {
    val fake = recordingClient(/* replies with a session row */)
    runBlocking {
        fake.client.rewindConversation(7L, "fork", "aaaaaaaa-0000-0000-0000-000000000002")
    }
    assertEquals("rewind_conversation", fake.lastTool)
    assertEquals(7L, fake.lastArgs["session_id"]?.jsonPrimitive?.long)
    assertEquals("fork", fake.lastArgs["mode"]?.jsonPrimitive?.content)
    assertEquals(
        "aaaaaaaa-0000-0000-0000-000000000002",
        fake.lastArgs["anchor_uuid"]?.jsonPrimitive?.content,
    )
}

@Test
fun aNullAnchorIsSentAsNullNotOmitted() {
    // Forking the newest turn has no later prompt, and null is the hub's word
    // for "keep the whole transcript" — omitting the key would mean the same
    // thing today but relies on the hub's default rather than saying it.
    val fake = recordingClient(/* replies with a session row */)
    runBlocking { fake.client.rewindConversation(7L, "fork", null) }
    assertTrue(fake.lastArgs.containsKey("anchor_uuid"))
}
```

Add to `HubContractTest.kt`:

```kotlin
@Test
fun theRewindGateRejectsHubsBelowTheFloor() {
    assertFalse(semverAtLeast("0.2.34", HUB_VERSION_REWIND))
    assertTrue(semverAtLeast(HUB_VERSION_REWIND, HUB_VERSION_REWIND))
    assertFalse(semverAtLeast(null, HUB_VERSION_REWIND))
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
./gradlew :shared:jvmTest --tests '*HubClientTest*' --tests '*HubContractTest*'
```

Expected: FAIL to compile — `unresolved reference: rewindConversation`, `HUB_VERSION_REWIND`.

- [ ] **Step 3: Find the real floor version**

```bash
# in the claude-fleet checkout
gh release list --limit 5
```

Take the first release that contains the `rewind_conversation` tool. Do **not** guess a number — the gate is worthless if the floor is wrong in either direction.

- [ ] **Step 4: Add the constant**

In `shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubContract.kt`, after `HUB_VERSION_KEYS`:

```kotlin
/**
 * The first hub release that serves `rewind_conversation` — the tool behind
 * the reply actions' Fork here, Rewind here and Retry. Additive, so the
 * contract revision does not move for it; gated on the hub's own version
 * string via [semverAtLeast], exactly as [HUB_VERSION_KEYS] is.
 *
 * Gated on the version rather than on a missing `prompt_uuid`, because a
 * missing anchor legitimately means "keep the whole transcript" for a fork of
 * the newest turn. Absence cannot double as "unsupported".
 */
const val HUB_VERSION_REWIND: String = "<from Step 3>"
```

- [ ] **Step 5: Add the client call**

In `shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubClient.kt`, after `sendKeys`:

```kotlin
    /**
     * Truncate a session's transcript into a new conversation and act on it.
     *
     * [mode] is `"fork"` (start a new session from that point, leaving this
     * one running) or `"rewind"` (restart THIS session there). [anchorUuid]
     * is the turn's own `promptUuid` for a rewind and the NEXT later turn's
     * for a fork; `null` keeps the whole transcript, which is what forking
     * the newest turn means. The original transcript is never changed.
     */
    suspend fun rewindConversation(
        sessionId: Long,
        mode: String,
        anchorUuid: String?,
        newWorktree: String? = null,
    ): SessionRow =
        call(
            "rewind_conversation",
            buildJsonObject {
                put("session_id", sessionId)
                put("mode", mode)
                put("anchor_uuid", anchorUuid)
                put("new_worktree", newWorktree)
            },
        ) { json.decodeFromJsonElement(SessionRow.serializer(), it) }
```

Add `"rewind_conversation"` to `LIFECYCLE_TOOLS` at `:763` if that set is what marks a tool as needing a `full` token — check what it actually gates before adding, and follow it.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
./gradlew :shared:jvmTest --tests '*HubClientTest*' --tests '*HubContractTest*'
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add shared/src/commonMain/kotlin/dev/claudefleet/mobile/net shared/src/commonTest/kotlin/dev/claudefleet/mobile/net
git commit -m "feat(net): call rewind_conversation, gated on the hub version

Gated on the version and not on a missing prompt_uuid: a missing anchor
legitimately means \"keep the whole transcript\" when forking the newest
turn, so absence cannot double as \"unsupported\".

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 3: The which-buttons rule, in Kotlin

The same rule the desktop has, against the same cases, so the two platforms cannot drift.

**Files:**
- Create: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/ui/ReplyActions.kt` (pure logic only, no Compose)
- Test: Create `shared/src/commonTest/kotlin/dev/claudefleet/mobile/ui/ReplyActionsTest.kt`

**Interfaces:**
- Consumes: `ConvTurn.promptUuid` (Task 1).
- Produces:
  ```kotlin
  data class ReplyActionsView(
      val canFork: Boolean, val canRewind: Boolean,
      val forkAnchor: String?, val rewindAnchor: String?,
  )
  fun replyActionsFor(
      turns: List<ConvTurn>, index: Int, truncated: Boolean, supported: Boolean,
  ): ReplyActionsView
  fun quoteText(text: String): String
  ```
  Task 4's composable calls both.

- [ ] **Step 1: Write the failing tests**

Create `shared/src/commonTest/kotlin/dev/claudefleet/mobile/ui/ReplyActionsTest.kt`. These are the desktop's nine cases, one for one — if a case is dropped here the platforms drift silently.

```kotlin
package dev.claudefleet.mobile.ui

import dev.claudefleet.mobile.model.ConvTurn
import kotlin.test.*

private fun turn(promptUuid: String?) =
    ConvTurn(prompt = if (promptUuid != null) "hi" else null, promptUuid = promptUuid)

class ReplyActionsTest {
    @Test
    fun forksOnTheNextLaterAnchorSoThisTurnIsKept() {
        val v = replyActionsFor(listOf(turn("a1"), turn("a2")), 0, truncated = false, supported = true)
        assertEquals("a2", v.forkAnchor)
        assertTrue(v.canFork)
    }

    @Test
    fun forkingTheNewestTurnKeepsTheWholeTranscript() {
        val v = replyActionsFor(listOf(turn("a1"), turn("a2")), 1, truncated = false, supported = true)
        assertTrue(v.canFork)
        assertNull(v.forkAnchor)
    }

    @Test
    fun skipsPromptlessTurnsScanningForwardForTheForkAnchor() {
        val v = replyActionsFor(listOf(turn("a1"), turn(null), turn("a3")), 0, truncated = false, supported = true)
        assertEquals("a3", v.forkAnchor)
    }

    @Test
    fun rewindsOnItsOwnAnchor() {
        val v = replyActionsFor(listOf(turn("a1"), turn("a2")), 1, truncated = false, supported = true)
        assertTrue(v.canRewind)
        assertEquals("a2", v.rewindAnchor)
    }

    @Test
    fun noRewindOnTheFirstTurnOfAnUntruncatedConversation() {
        val v = replyActionsFor(listOf(turn("a1"), turn("a2")), 0, truncated = false, supported = true)
        assertFalse(v.canRewind)
    }

    @Test
    fun rewindIsOfferedAtIndexZeroWhenTheWindowIsTruncated() {
        val v = replyActionsFor(listOf(turn("a1"), turn("a2")), 0, truncated = true, supported = true)
        assertTrue(v.canRewind)
        assertEquals("a1", v.rewindAnchor)
    }

    @Test
    fun noRewindOnAPromptlessTurn() {
        val v = replyActionsFor(listOf(turn("a1"), turn(null)), 1, truncated = false, supported = true)
        assertFalse(v.canRewind)
    }

    @Test
    fun anUnsupportedHubOffersNeither() {
        val v = replyActionsFor(listOf(turn("a1"), turn("a2")), 1, truncated = false, supported = false)
        assertFalse(v.canFork)
        assertFalse(v.canRewind)
    }

    @Test
    fun quotePrefixesEveryLineIncludingBlankOnes() {
        assertEquals("> a\n>\n> b\n\n", quoteText("a\n\nb"))
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
./gradlew :shared:jvmTest --tests '*ReplyActionsTest*'
```

Expected: FAIL to compile — `unresolved reference: replyActionsFor`.

- [ ] **Step 3: Write the logic**

Create `shared/src/commonMain/kotlin/dev/claudefleet/mobile/ui/ReplyActions.kt`:

```kotlin
package dev.claudefleet.mobile.ui

import dev.claudefleet.mobile.model.ConvTurn

/** Which of the five reply actions a turn offers, and with what anchor. */
data class ReplyActionsView(
    val canFork: Boolean,
    /** Gates Rewind here AND Retry — Retry is a rewind plus a re-send. */
    val canRewind: Boolean,
    /** Keep strictly before this; null keeps the whole transcript. */
    val forkAnchor: String?,
    val rewindAnchor: String?,
)

/**
 * The desktop has the same function over the same cases
 * (`src/lib/reply_actions.ts` in `claude-fleet`). Keep the two test suites in
 * step: a case dropped on one platform is a silent divergence, not a gap a
 * compiler will find.
 *
 * [truncated] is the conversation's own flag — index 0 is the conversation's
 * FIRST turn only when nothing older was dropped, and rewinding the first turn
 * would leave an empty conversation, which is `/clear` under a misleading name.
 * [supported] is the hub-version gate.
 */
fun replyActionsFor(
    turns: List<ConvTurn>,
    index: Int,
    truncated: Boolean,
    supported: Boolean,
): ReplyActionsView {
    if (!supported) {
        return ReplyActionsView(canFork = false, canRewind = false, forkAnchor = null, rewindAnchor = null)
    }
    // Fork keeps everything through THIS turn, so it anchors on the next later
    // prompt. Prompt-less turns carry no anchor, so scan past them: keeping
    // more history is safe, keeping less would silently discard work.
    val forkAnchor = turns.drop(index + 1).firstNotNullOfOrNull { it.promptUuid }

    val own = turns.getOrNull(index)?.promptUuid
    val isConversationStart = index == 0 && !truncated
    val canRewind = own != null && !isConversationStart

    return ReplyActionsView(
        canFork = true,
        canRewind = canRewind,
        forkAnchor = forkAnchor,
        rewindAnchor = if (canRewind) own else null,
    )
}

/** A reply as a Markdown block quote, ready to precede the user's own words. */
fun quoteText(text: String): String =
    text.split("\n").joinToString("\n") { if (it.isEmpty()) ">" else "> $it" } + "\n\n"
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
./gradlew :shared:jvmTest --tests '*ReplyActionsTest*'
```

Expected: PASS, all nine.

- [ ] **Step 5: Commit**

```bash
git add shared/src/commonMain/kotlin/dev/claudefleet/mobile/ui/ReplyActions.kt shared/src/commonTest/kotlin/dev/claudefleet/mobile/ui/ReplyActionsTest.kt
git commit -m "feat(ui): the which-buttons rule for reply actions

The desktop's nine cases, one for one, so the two platforms cannot drift
without a test going red.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
```

---

### Task 4: The action row on screen

**Files:**
- Modify: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/ui/SessionScreen.kt:848` (`Turn`), `:873` (`Item`, the `ConvItem.Text` branch), and `:369` (`turnItems`, to pass the turn's index, the turn list and `truncated` down)
- Modify: `shared/src/commonMain/kotlin/dev/claudefleet/mobile/ui/SessionViewModel.kt` (a `rewind`/`fork` action, following how it exposes `sendPrompt`)
- Test: `shared/src/commonTest/kotlin/dev/claudefleet/mobile/ui/SessionViewModelTest.kt`; `androidApp/src/androidTest/kotlin/dev/claudefleet/mobile/android/ConversationItemsTest.kt`

**Interfaces:**
- Consumes: `replyActionsFor`, `quoteText` (Task 3); `HubClient.rewindConversation`, `HUB_VERSION_REWIND` (Task 2).
- Produces: nothing downstream — this is the last task.

- [ ] **Step 1: Write the failing view-model test**

Add to `shared/src/commonTest/kotlin/dev/claudefleet/mobile/ui/SessionViewModelTest.kt`, in the file's existing fake-client style:

```kotlin
@Test
fun retryRewindsThenSendsTheSamePrompt() {
    // Retry is not a third mode: it is a rewind followed by a send, so it
    // inherits the hub's refusals instead of keeping a second copy of them.
    val vm = viewModelWith(/* a conversation of two prompted turns */)
    runBlocking { vm.retryTurn(index = 1) }
    assertEquals(listOf("rewind_conversation", "send_prompt"), fake.toolsCalled)
    assertEquals("rewind", fake.argsFor("rewind_conversation")["mode"]?.jsonPrimitive?.content)
}

@Test
fun aFailedRewindDoesNotThenSendThePrompt() {
    // Otherwise a refused rewind (mid-turn, say) would append the prompt to
    // the conversation it failed to rewind — the worst of both outcomes.
    val vm = viewModelWith(/* … */, rewindFails = true)
    runBlocking { vm.retryTurn(index = 1) }
    assertEquals(listOf("rewind_conversation"), fake.toolsCalled)
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
./gradlew :shared:jvmTest --tests '*SessionViewModelTest*'
```

Expected: FAIL to compile — `unresolved reference: retryTurn`.

- [ ] **Step 3: Add the view-model actions**

In `SessionViewModel.kt`, following how `sendPrompt` is exposed (client access, error surfacing, state refresh):

```kotlin
    /** Rewind to before [index]'s prompt, then send that prompt again. */
    suspend fun retryTurn(index: Int) {
        val view = replyActionsFor(turns, index, truncated, rewindSupported)
        val anchor = view.rewindAnchor ?: return
        val prompt = turns.getOrNull(index)?.prompt ?: return
        // A failed rewind must NOT fall through to the send: appending the
        // prompt to the conversation we failed to rewind is the worst outcome.
        if (!rewind(anchor)) return
        sendPrompt(prompt)
    }
```

Add `rewind(anchor)` returning `Boolean`, and `fork(anchor, newWorktree)`; both call `HubClient.rewindConversation`. `rewindSupported` is `semverAtLeast(hubVersion, HUB_VERSION_REWIND) && tokenIsFull`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
./gradlew :shared:jvmTest --tests '*SessionViewModelTest*'
```

Expected: PASS.

- [ ] **Step 5: Write the composable**

Add to `SessionScreen.kt`, next to `Item`:

```kotlin
/**
 * The action row under one reply. Always visible: on a phone there is no
 * hover, so a hidden control is no control at all.
 */
@Composable
private fun ReplyActions(
    view: ReplyActionsView,
    text: String,
    onQuote: (String) -> Unit,
    onRetry: () -> Unit,
    onFork: (String?) -> Unit,
    onRewind: (String) -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    Row(modifier = Modifier.fillMaxWidth().padding(top = 4.dp)) {
        TextButton(onClick = { clipboard.setText(AnnotatedString(text)) }) { Text("Copy") }
        TextButton(onClick = { onQuote(quoteText(text)) }) { Text("Quote") }
        if (view.canRewind) TextButton(onClick = onRetry) { Text("Retry") }
        if (view.canFork) TextButton(onClick = { onFork(view.forkAnchor) }) { Text("Fork here") }
        if (view.canRewind) {
            TextButton(onClick = { onRewind(view.rewindAnchor!!) }) { Text("Rewind here") }
        }
    }
}
```

Thread `turns`, `index` and `truncated` from `turnItems` (`:369`) through `Turn` (`:848`) into the `ConvItem.Text` branch of `Item` (`:873`), and render `ReplyActions` after the `MarkdownText`. `turnItems`' existing "keyed by position" comment already documents that the index is stable — no change needed there beyond passing it.

- [ ] **Step 6: Add the confirmations and the fork sheet**

Retry and Rewind open an `AlertDialog` whose body is, verbatim:

> The conversation is rewound to before this turn. Your files are left as they are.

Retry appends "The same prompt is then sent again."; Rewind appends "The prompt returns to the composer."

Fork opens a `ModalBottomSheet` with two radio options — **New worktree** (selected by default, with a name field) and **Same worktree**, whose label reads "⚠ both sessions edit the same files". The sheet is Fork's confirmation; there is no second dialog. Follow the sheet idiom in `TodaySheet.kt`.

- [ ] **Step 7: Add the UI test**

In `androidApp/src/androidTest/kotlin/dev/claudefleet/mobile/android/ConversationItemsTest.kt`:

```kotlin
@Test
fun theFirstTurnOfAnUntruncatedConversationOffersNoRewind() {
    // …render a session with two prompted turns, truncated = false…
    composeRule.onAllNodesWithText("Copy").assertCountEquals(2)
    composeRule.onAllNodesWithText("Rewind here").assertCountEquals(1)
}
```

- [ ] **Step 8: Run everything**

```bash
./gradlew :shared:jvmTest
./gradlew build
```

Expected: PASS.

- [ ] **Step 9: Commit and open the PR**

```bash
git add -A
git commit -m "feat(ui): reply actions on the phone

Copy, Quote, Retry, Fork here, Rewind here under every reply, against the
hub's rewind_conversation. Destructive actions are gated twice: on the hub
version and on the token being full.

Claude-Session: https://claude.ai/code/session_012PBjHSukJW9dyjDaEPpvDy"
gh pr create --fill
```

---

## Self-Review

**Spec coverage.** §1's five buttons → Tasks 3, 4. §3's anchor → Task 1, and its gating table → Task 3's nine tests. §5.1's "always visible" → Task 4's composable and its comment. §5.2's fork sheet → Task 4 Step 6. §5.3's copy → Task 4 Step 6, verbatim. §6's tool → Task 2. §7's mobile tests → Tasks 1, 3, 4. §9's ordering → the prerequisite at the top.

The phone builds no engine and needs no verdict row, no budget raise and no regen: every one of those lives in the `claude-fleet` plan, where the tool is defined.

**A case this plan surfaced that the desktop plan was missing:** "a failed rewind does not then send the prompt" (Task 4 Step 1). Retry is two calls on both platforms, so the desktop had the same hazard — `doRewind` returned early on `!r.ok` by construction but nothing held it there. The fleet plan's Task 6 Step 7 now carries the equivalent test (`a refused rewind does NOT then send the prompt`), so both platforms keep the guard on purpose rather than by accident.

**Type consistency.** `promptUuid` (Kotlin) ↔ `prompt_uuid` (wire) ↔ `prompt_uuid` (Rust/TS) are consistent. `ReplyActionsView`'s four properties match the desktop's four, same names. `replyActionsFor`'s four parameters are in the same order on both platforms. `rewindConversation(sessionId, mode, anchorUuid, newWorktree)` matches its call in Task 4's `rewind`/`fork`. `mode` is the same closed set of strings, `"rewind"` / `"fork"`.

**Placeholders.** Task 2 Step 4 contains `"<from Step 3>"` deliberately — the floor must be a real released version, and Step 3 is the command that finds it; inventing a number here is the one failure mode a gate cannot survive. Task 4 Steps 6 and 7 describe structure rather than every line, because the dialog and sheet idioms must match the repo's existing ones; each names the file to copy from and the exact copy and defaults that must result. Test bodies marked with `/* … */` are where this plan cannot know the repo's current fake-client helper names — each says which existing test file to follow, and the assertions themselves are complete.
