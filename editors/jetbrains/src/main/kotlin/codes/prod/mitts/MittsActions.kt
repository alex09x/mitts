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

import com.intellij.execution.ProgramRunnerUtil
import com.intellij.execution.RunManager
import com.intellij.execution.RunnerAndConfigurationSettings
import com.intellij.execution.executors.DefaultRunExecutor
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project

/** Tools → Mitts → Check, Build, ...: runs `mitts <command>` in the project's directory. */
abstract class MittsCommandAction(private val command: String) : AnAction(), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        ProgramRunnerUtil.executeConfiguration(
            configuration(project, command),
            DefaultRunExecutor.getRunExecutorInstance(),
        )
    }

    override fun update(e: AnActionEvent) {
        e.presentation.isEnabled = e.project != null
    }

    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    companion object {
        /** A temporary `mitts <command>` run configuration, selected in the run widget. */
        fun configuration(project: Project, command: String): RunnerAndConfigurationSettings {
            val runManager = RunManager.getInstance(project)
            val name = "mitts $command"
            val settings = runManager.findConfigurationByTypeAndName(MittsConfigurationType.ID, name)
                ?: runManager.createConfiguration(
                    name,
                    MittsConfigurationType.getInstance().configurationFactories.single(),
                ).also {
                    (it.configuration as MittsRunConfiguration).command = command
                    it.isTemporary = true
                    runManager.addConfiguration(it)
                }
            runManager.selectedConfiguration = settings
            return settings
        }
    }
}

class MittsCheckAction : MittsCommandAction("check")
class MittsBuildAction : MittsCommandAction("build")
class MittsTestAction : MittsCommandAction("test")
class MittsRunAction : MittsCommandAction("run")
class MittsClippyAction : MittsCommandAction("clippy")
