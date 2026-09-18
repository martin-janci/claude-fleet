# fleet-mobile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Kotlin Multiplatform app with one shared Compose UI that pairs with a `fleet-hub`, lists every session across the fleet, shows a session's conversation, sends prompts, and updates live.

**Architecture:** A `shared` KMP module holds models, the hub client, the repository and the whole Compose UI; `androidApp` and `iosApp` are thin hosts. Ktor speaks JSON-RPC `tools/call` over `POST /mcp` and subscribes to `GET /events`; a repository applies event frames to an in-memory snapshot exposed as `StateFlow`s. Secure storage and the camera are the only `expect`/`actual` pairs.

**Tech Stack:** Kotlin 2.x, Compose Multiplatform, Ktor client (OkHttp/Darwin engines, SSE), kotlinx-serialization, kotlinx-coroutines, AndroidX Security for storage, CameraX + ML Kit barcode on Android, AVFoundation on iOS, Gradle with a version catalog, JUnit on the JVM.

**Spec:** `docs/superpowers/specs/2026-09-18-fleet-mobile-design.md` in the `claude-fleet` repo (copied into this repo's `docs/` by Task 8).

## Global Constraints

- The repository is **new and separate**: `/home/dev/projects/github.com/martin-janci/fleet-mobile`. Nothing in `claude-fleet` changes. `git init` it in Task 1; it is pushed only in Task 8.
- The Android SDK is at `/home/dev/Android/Sdk` (platform 35, build-tools 35.0.0, platform-tools). Export `ANDROID_HOME` and `ANDROID_SDK_ROOT` before any Gradle invocation. JDK 21 is on the PATH.
- iOS **cannot be built here** (no macOS, no Xcode). Configure the target, keep the code compiling for it by construction, and leave the build to the developer's Mac. Never claim an iOS build was verified.
- Every task ends green on `./gradlew :shared:jvmTest` and, from Task 7 on, `./gradlew :androidApp:assembleDebug`.
- No secret in the repository: no token, no hub URL, no hostname. Examples use `https://fleet.example.com`.
- The app calls only tools a client token may use: `list_sessions`, `list_hosts`, `list_projects`, `session_conversation`, `send_prompt`, `capture_session`, `session_history`, `wait_for_session`, `fleet_health`. It must never call `provision_hosts`, `add_host`, `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `pair_client`, `list_clients` or `revoke_client` — the hub refuses those to a client, and a refusal in the app is a bug.
- The hub's `POST /mcp` answers **SSE-framed**: the JSON-RPC body arrives on a `data:` line. A tool failure comes back as a *result* with `isError: true` and an `E_*` code in `structuredContent`, not as a JSON-RPC error. Both shapes must be handled.
- Commit per task with a Conventional Commit message.

## File map

| Path | Responsibility |
|---|---|
| `settings.gradle.kts`, `build.gradle.kts`, `gradle/libs.versions.toml`, `gradle.properties` | Build, plugins, version catalog |
| `shared/src/commonMain/kotlin/.../model/` | `SessionRow`, `HostRow`, `ProjectRow`, `Conversation`, `ConvTurn`, `ConvItem`, `FleetEvent` |
| `shared/src/commonMain/kotlin/.../net/HubClient.kt` | JSON-RPC envelope, SSE `data:` unwrapping, error mapping, `/pair` |
| `shared/src/commonMain/kotlin/.../net/EventStream.kt` | `GET /events` subscription, reconnect with backoff |
| `shared/src/commonMain/kotlin/.../data/FleetRepository.kt` | Snapshot, frame application, `StateFlow`s, refresh |
| `shared/src/commonMain/kotlin/.../store/Secrets.kt` (+ `androidMain`, `iosMain`) | `expect`/`actual` secure storage |
| `shared/src/commonMain/kotlin/.../ui/` | Compose screens: Pair, Sessions, Session, Hosts, Settings |
| `shared/src/commonMain/kotlin/.../ui/scan/QrScanner.kt` (+ actuals) | `expect`/`actual` camera scanner |
| `shared/src/commonTest/kotlin/` | Client, repository, event and flow tests against a Ktor `MockEngine` |
| `androidApp/` | `MainActivity`, manifest, permissions, icons |
| `iosApp/` | SwiftUI host embedding the shared UI |
| `.github/workflows/ci.yml` | `:shared:jvmTest` + `:androidApp:assembleDebug` |
| `README.md`, `docs/` | How to build, how to pair, what the app may do |

---

### Task 1: Repository skeleton that builds

**Files:** `settings.gradle.kts`, `build.gradle.kts`, `gradle/libs.versions.toml`, `gradle.properties`, `gradlew` (wrapper), `shared/build.gradle.kts`, `androidApp/build.gradle.kts`, `.gitignore`, `local.properties` (git-ignored, pointing at the SDK)

**Interfaces:** Produces a Gradle project where `:shared` has `commonMain`/`commonTest`/`androidMain`/`iosMain` source sets and `:androidApp` produces a debug APK.

- [ ] **Step 1: Create the project and the wrapper**

```bash
mkdir -p /home/dev/projects/github.com/martin-janci/fleet-mobile
cd /home/dev/projects/github.com/martin-janci/fleet-mobile && git init -b main
export ANDROID_HOME=$HOME/Android/Sdk ANDROID_SDK_ROOT=$HOME/Android/Sdk
# Gradle is not installed: download the wrapper once from services.gradle.org,
# then use ./gradlew from here on.
```

Pin the Gradle version in `gradle/wrapper/gradle-wrapper.properties` and check the wrapper in, as every Gradle project does.

- [ ] **Step 2: Write the build files**

A version catalog holds Kotlin, AGP, Compose Multiplatform, Ktor, coroutines and serialization versions. `:shared` applies the KMP, Compose and serialization plugins with `androidTarget()`, `jvm()` (so `commonTest` runs on the JVM without a device), and the three iOS targets. `:androidApp` applies AGP with `compileSdk = 35`, `minSdk = 26`, `targetSdk = 35`.

- [ ] **Step 3: One placeholder test, to prove the toolchain**

```kotlin
// shared/src/commonTest/kotlin/ToolchainTest.kt
class ToolchainTest {
    @Test fun the_shared_module_compiles_and_tests_run() {
        assertEquals(4, 2 + 2)
    }
}
```

- [ ] **Step 4: Verify**

Run: `./gradlew :shared:jvmTest :androidApp:assembleDebug`
Expected: both succeed; the APK exists under `androidApp/build/outputs/apk/debug/`.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "chore: Kotlin Multiplatform skeleton that builds for Android"
```

---

### Task 2: Models and the hub client

**Files:** `shared/src/commonMain/kotlin/.../model/*.kt`, `.../net/HubClient.kt`, `shared/src/commonTest/kotlin/net/HubClientTest.kt`

**Interfaces:**

```kotlin
class HubClient(private val http: HttpClient, private val base: String, private val token: String?) {
    suspend fun <T> call(tool: String, args: JsonObject, deserialize: (JsonElement) -> T): T
    suspend fun pair(code: String): PairResult           // POST /pair
    suspend fun listSessions(): List<SessionRow>
    suspend fun listHosts(): List<HostRow>
    suspend fun conversation(sessionId: Long, turns: Int? = null): Conversation
    suspend fun sendPrompt(sessionId: Long, text: String): SendPromptResult
}
sealed class HubError : Exception() {
    data class Unauthorized(...) : HubError()      // 401
    data class Forbidden(...) : HubError()         // 403, carries the body
    data class Tool(val code: String, val message: String) : HubError()  // isError result
    data class Transport(val cause: Throwable) : HubError()
}
```

- [ ] **Step 1: Write the failing tests** against a Ktor `MockEngine`, covering: a tool result arriving on an SSE `data:` line is parsed; a result with `isError: true` becomes `HubError.Tool` with the `E_*` code; a JSON-RPC `error` object becomes `HubError.Tool` too; `401` becomes `Unauthorized`; `403` becomes `Forbidden` carrying the body; a connection failure becomes `Transport`; `pair` posts `{"code": ...}` to `/pair` and reads back the token and hub URL; the `Authorization` header is present on `/mcp` and absent on `/pair`.

- [ ] **Step 2: Run to verify they fail** — `./gradlew :shared:jvmTest`

- [ ] **Step 3: Implement** the models (mirroring the hub's JSON: `SessionRow` with `id`, `tmux_name`, `friendly_name`, `host_alias`, `project_id`, `claude_status`, `stuck_kind`, `current_activity`, `last_activity_at`; `HostRow`; `Conversation`/`ConvTurn`/`ConvItem` with the `kind`-tagged item shape) and the client. Use `@SerialName` for snake_case, and `ignoreUnknownKeys = true` so a hub that grows a field does not break the app.

- [ ] **Step 4: Verify and commit** — `./gradlew :shared:jvmTest`, then `feat(net): hub client over JSON-RPC with the hub's SSE framing`

---

### Task 3: Pairing and secure storage

**Files:** `.../store/Secrets.kt` + `androidMain`/`iosMain` actuals, `.../data/Session.kt` (the app's own "am I paired" state), tests

**Interfaces:**

```kotlin
expect class Secrets { 
    suspend fun read(): Credentials?     // hub base URL + token + client name
    suspend fun write(c: Credentials)
    suspend fun clear()
}
```

- [ ] **Step 1: Failing tests** for the pairing flow against a fake `Secrets` and a `MockEngine`: a scanned URL of the form `https://hub.example.com/pair#ABCD1234` yields the code and the base; pairing stores the credentials; a `401` from any later call clears them; `clear()` leaves nothing readable.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement** the common logic plus the Android actual (`EncryptedSharedPreferences`) and the iOS actual (Keychain via `platform.Security`). Parse both a full pair URL and a bare code.
- [ ] **Step 4: Verify and commit** — `feat(pair): scan or paste a code, store the credential securely`

---

### Task 4: The repository and the live event stream

**Files:** `.../data/FleetRepository.kt`, `.../net/EventStream.kt`, tests

**Interfaces:**

```kotlin
class FleetRepository(client: HubClient, events: EventStream, scope: CoroutineScope) {
    val sessions: StateFlow<List<SessionRow>>
    val hosts: StateFlow<List<HostRow>>
    val status: StateFlow<ConnectionStatus>   // Connected, Reconnecting(attempt), Offline(reason)
    suspend fun refresh()
    fun start() ; fun stop()
}
```

- [ ] **Step 1: Failing tests:** a `session:updated` frame replaces a row by id; `session:killed` removes it; an unknown event name is ignored; a `lagged` frame triggers a refetch; a dropped stream reconnects with growing backoff and the status flow reports it; `stop()` cancels everything.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.** The stream parses `event:`/`data:` pairs; the repository applies them to the snapshot. Keep the frame-application logic pure and separately tested.
- [ ] **Step 4: Verify and commit** — `feat(data): live snapshot driven by the hub's event stream`

---

### Task 5: Sessions and Session screens

**Files:** `.../ui/SessionsScreen.kt`, `.../ui/SessionScreen.kt`, `.../ui/components/*.kt`, view models, tests for the view models

- [ ] **Step 1: Failing view-model tests:** sessions group by host then project; the "needs attention" filter keeps only blocked or stuck rows; sending a prompt disables the box until the call returns and surfaces a `HubError.Tool` message; the conversation loads on open and appends on refresh.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement** the view models and the Compose screens. Sessions: grouped list, status chip, the hub's one-line activity, a filter toggle. Session: turns newest at the bottom, tool calls as one-line summaries with a failure marker, a prompt box, and the session's status and host in the bar.
- [ ] **Step 4: Verify and commit** — `feat(ui): the fleet list and a session's conversation`

---

### Task 6: Pair, Hosts and Settings screens

**Files:** `.../ui/PairScreen.kt`, `.../ui/HostsScreen.kt`, `.../ui/SettingsScreen.kt`, `.../ui/scan/QrScanner.kt` + actuals, navigation

- [ ] **Step 1: Failing tests** for the pair view model (a scan result that is not a fleet pair URL is rejected with a message; a successful pair emits Paired) and the settings view model (forget clears credentials and returns to Pair; the screen never offers revocation, which is the operator's).
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Implement.** Navigation between the five screens. The scanner is `expect`/`actual`: CameraX + ML Kit on Android, `AVCaptureSession` on iOS, with a manual-entry field always available so the app is usable without the camera permission.
- [ ] **Step 4: Verify and commit** — `feat(ui): pairing, hosts and settings`

---

### Task 7: The Android app host

**Files:** `androidApp/src/main/AndroidManifest.xml`, `MainActivity.kt`, icons, `androidApp/build.gradle.kts`

- [ ] **Step 1:** Wire the Compose entry point to the shared UI; declare the camera and internet permissions; ask for the camera only when the scanner opens.
- [ ] **Step 2:** Verify — `./gradlew :androidApp:assembleDebug`, and report the APK's size and path.
- [ ] **Step 3:** Commit — `feat(android): the app host, permissions and icons`

---

### Task 8: iOS host, CI, docs, and the repository

**Files:** `iosApp/`, `.github/workflows/ci.yml`, `README.md`, `docs/`

- [ ] **Step 1:** A SwiftUI host embedding the shared UI, plus the Xcode project files, written to be correct by construction and marked explicitly as unbuilt here.
- [ ] **Step 2:** CI on Linux: `./gradlew :shared:jvmTest :androidApp:assembleDebug` with the Android SDK action.
- [ ] **Step 3:** `README.md`: what the app is, how to pair (`fleet-hub pair --name phone` on the hub, scan), what it deliberately cannot do, how to build each platform, and that iOS needs a Mac. Copy the design doc into `docs/`.
- [ ] **Step 4:** Verify everything once more, then commit — `docs: how to build and pair`. Do NOT create the GitHub repository or push; the controller does that.

---

## Self-review

**Spec coverage.** Shared Compose UI → Tasks 5–7. Pairing by scan → Tasks 3 and 6. Fleet list → Task 5. Conversation and prompts → Task 5. Live updates → Task 4. Secure storage → Task 3. The refusal and failure paths → Task 2's error mapping, exercised in Tasks 4–6. iOS configured but unbuilt → Tasks 1 and 8, stated plainly.

**Placeholders.** None: every task names its files, its tests and its verification command. Task 1's wrapper download is a one-time bootstrap and is written as such.

**Type consistency.** `HubClient`, `HubError`, `Secrets`, `Credentials`, `FleetRepository` and the model names are used identically across Tasks 2–6. The tool list in the constraints matches the calls in Task 2's interface.
