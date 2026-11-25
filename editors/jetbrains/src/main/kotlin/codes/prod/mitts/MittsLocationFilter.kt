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

import com.intellij.execution.filters.ConsoleFilterProvider
import com.intellij.execution.filters.Filter
import com.intellij.execution.filters.OpenFileHyperlinkInfo
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import java.nio.file.InvalidPathException
import java.nio.file.Path

/**
 * Links the location line of a rustc diagnostic in Mitts' output (` --> src/main.rs:2:18`)
 * to the file. Relative paths are relative to `baseDirectory`, the directory Mitts ran in.
 */
class MittsLocationFilter(
    private val project: Project,
    private val baseDirectory: String?,
) : Filter, DumbAware {

    override fun applyFilter(line: String, entireLength: Int): Filter.Result? {
        val match = PATTERN.find(line) ?: return null
        val (path, lineNumber, column) = match.destructured
        val file = resolve(path) ?: return null
        val lineStart = entireLength - line.length
        val start = lineStart + match.groups[1]!!.range.first
        val end = lineStart + match.groups[3]!!.range.last + 1
        val info = OpenFileHyperlinkInfo(project, file, lineNumber.toInt() - 1, column.toInt() - 1)
        return Filter.Result(start, end, info)
    }

    private fun resolve(path: String): VirtualFile? {
        val target = try {
            val p = Path.of(path)
            if (p.isAbsolute) p else Path.of(baseDirectory ?: return null).resolve(p)
        } catch (_: InvalidPathException) {
            return null
        }.normalize()
        val fs = LocalFileSystem.getInstance()
        return fs.findFileByNioFile(target) ?: fs.refreshAndFindFileByNioFile(target)
    }

    companion object {
        val PATTERN = Regex("""^\s*-->\s*(.+?):(\d+):(\d+)\s*$""")
    }
}

/** Every console of a project links rustc locations relative to the project's directory. */
class MittsFilterProvider : ConsoleFilterProvider {
    override fun getDefaultFilters(project: Project): Array<Filter> =
        arrayOf(MittsLocationFilter(project, project.basePath))
}
