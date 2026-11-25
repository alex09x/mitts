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

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.util.execution.ParametersListUtil
import java.nio.charset.StandardCharsets

object MittsCommand {
    /** `executable` with the arguments of `command` (a shell-like line: `test -p a`). */
    fun commandLine(
        command: String,
        workingDirectory: String?,
        executable: String = MittsSettings.getInstance().executable,
    ): GeneralCommandLine =
        GeneralCommandLine(listOf(executable) + ParametersListUtil.parse(command))
            .withWorkingDirectory(workingDirectory?.takeIf { it.isNotBlank() }?.let { java.nio.file.Path.of(it) })
            .withCharset(StandardCharsets.UTF_8)
            .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
}
