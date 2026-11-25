/*
 * Mitts Build for JetBrains IDEs — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

package codes.prod.mitts

import com.intellij.execution.filters.OpenFileHyperlinkInfo
import com.intellij.execution.process.CapturingProcessHandler
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.util.io.FileUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import java.io.File
import java.nio.file.Files
import java.nio.file.Path

class MittsLocationFilterTest : BasePlatformTestCase() {

    /** (path relative to `base`, 1-based line, 1-based column) of each link in `output`. */
    private fun links(output: String, base: Path): List<Triple<String, Int, Int>> {
        val filter = MittsLocationFilter(project, base.toString())
        val found = mutableListOf<Triple<String, Int, Int>>()
        var offset = 0
        for (line in output.split('\n').map { "$it\n" }) {
            offset += line.length
            val result = filter.applyFilter(line, offset) ?: continue
            for (item in result.resultItems) {
                val text = line.substring(
                    item.highlightStartOffset - (offset - line.length),
                    item.highlightEndOffset - (offset - line.length),
                )
                assertTrue("link covers the location: $text", text.contains(':'))
                val descriptor = (item.hyperlinkInfo as OpenFileHyperlinkInfo).descriptor!!
                val relative = base.relativize(Path.of(descriptor.file.path)).toString().replace('\\', '/')
                // With the file's document at hand the descriptor carries an offset, not a line.
                val (lineIndex, columnIndex) = if (descriptor.line >= 0) {
                    descriptor.line to descriptor.column
                } else {
                    val document = FileDocumentManager.getInstance().getDocument(descriptor.file)!!
                    val l = document.getLineNumber(descriptor.offset)
                    l to descriptor.offset - document.getLineStartOffset(l)
                }
                found += Triple(relative, lineIndex + 1, columnIndex + 1)
            }
        }
        return found
    }

    private fun workspace(vararg files: String): Path {
        val root = FileUtil.createTempDirectory("mitts-ws", null).toPath().toRealPath()
        for (file in files) {
            val path = root.resolve(file)
            Files.createDirectories(path.parent)
            // Lines long enough for every column the tests name (a link to a column past
            // the end of its line lands at the line's end).
            Files.writeString(path, "// ${"x".repeat(60)}\n".repeat(5))
        }
        return root
    }

    fun testLinksEveryRustcLocationInCapturedMittsOutput() {
        val base = workspace("crates/a/src/main.rs", "crates/a/src/lib.rs")
        val output = javaClass.getResource("/check.txt")!!.readText()
        assertEquals(
            listOf(
                Triple("crates/a/src/main.rs", 2, 9),
                Triple("crates/a/src/lib.rs", 1, 27),
                Triple("crates/a/src/lib.rs", 3, 20),
            ),
            links(output, base),
        )
    }

    fun testSummariesAndMittsReportGiveNoLinks() {
        val base = workspace("crates/a/src/lib.rs")
        val output = """
            error: aborting due to 2 previous errors
               ╭─[$base/crates/a/src/lib.rs:1:27]
              × Failed to compile a v0.1.0 [lib:a]
        """.trimIndent()
        assertEquals(emptyList<Any>(), links(output, base))
    }

    fun testAFileThatDoesNotExistGivesNoLink() {
        val base = workspace()
        assertEquals(emptyList<Any>(), links(" --> src/missing.rs:1:1", base))
    }

    fun testAbsolutePathsAreLinkedAsTheyAre() {
        val base = workspace("src/main.rs")
        val other = workspace("src/lib.rs")
        val found = links(" --> ${other.resolve("src/lib.rs")}:4:2", base)
        assertEquals(1, found.size)
        assertEquals(4 to 2, found[0].second to found[0].third)
    }

    /** With MITTS_JB_E2E=1: the real `mitts check` on a broken project, through the filter. */
    fun testEndToEndRealMittsCheckIsLinked() {
        if (System.getenv("MITTS_JB_E2E") != "1") {
            println("e2e skipped: MITTS_JB_E2E is not 1")
            return
        }
        println("e2e: running mitts check")
        val fixture = File("src/test/fixtures/broken").canonicalFile.toPath()
        val handler = CapturingProcessHandler(MittsCommand.commandLine("check", fixture.toString(), "mitts"))
        val run = handler.runProcess(300_000)
        assertTrue("mitts check fails on the broken project: ${run.stderr}", run.exitCode != 0)
        assertEquals(
            listOf(Triple("src/main.rs", 2, 18)),
            links(run.stdout + run.stderr, fixture),
        )
    }
}
