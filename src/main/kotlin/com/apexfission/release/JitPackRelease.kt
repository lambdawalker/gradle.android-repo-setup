package com.apexfission.release

internal const val STABLE_VERSION = "(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)"

internal data class JitPackRelease(val module: String, val version: String) {
    val tag: String get() = "$module/v$version-jitpack"

    fun verifyCheckout(head: String, taggedCommit: String, reportedCommit: String?) {
        require(head == taggedCommit) { "Canonical JitPack tag does not identify the checkout" }
        require(reportedCommit == null || reportedCommit == head) { "JitPack GIT_COMMIT differs from HEAD" }
    }

    companion object {
        private val tagPattern = Regex("([A-Za-z0-9_-]+)[/~]v($STABLE_VERSION)-jitpack")

        fun parse(value: String): JitPackRelease {
            val match = requireNotNull(tagPattern.matchEntire(value)) {
                "VERSION must be a canonical module/vX.Y.Z-jitpack tag (slash or tilde)"
            }
            return JitPackRelease(match.groupValues[1], match.groupValues[2])
        }
    }
}
