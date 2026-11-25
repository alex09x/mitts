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

import com.intellij.execution.Executor
import com.intellij.execution.configurations.CommandLineState
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.ConfigurationTypeUtil
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.execution.configurations.RunConfigurationBase
import com.intellij.execution.configurations.RunConfigurationOptions
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.process.KillableColoredProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.icons.AllIcons
import com.intellij.openapi.components.BaseState
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent

class MittsConfigurationType : ConfigurationTypeBase(
    ID, "Mitts", "Run a Mitts command", AllIcons.Actions.Execute,
) {
    init {
        addFactory(MittsConfigurationFactory(this))
    }

    companion object {
        const val ID = "MittsRunConfiguration"

        fun getInstance(): MittsConfigurationType =
            ConfigurationTypeUtil.findConfigurationType(MittsConfigurationType::class.java)
    }
}

class MittsConfigurationFactory(type: MittsConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = "Mitts"

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        MittsRunConfiguration(project, this, "Mitts")

    override fun getOptionsClass(): Class<out BaseState> = MittsRunConfigurationOptions::class.java
}

class MittsRunConfigurationOptions : RunConfigurationOptions() {
    private val commandProperty = string("check").provideDelegate(this, "command")
    private val directoryProperty = string("").provideDelegate(this, "workingDirectory")

    var command: String
        get() = commandProperty.getValue(this) ?: ""
        set(value) = commandProperty.setValue(this, value)

    var workingDirectory: String
        get() = directoryProperty.getValue(this) ?: ""
        set(value) = directoryProperty.setValue(this, value)
}

/** `mitts <command>` in a directory (default: the project's), output linked to sources. */
class MittsRunConfiguration(project: Project, factory: ConfigurationFactory, name: String) :
    RunConfigurationBase<MittsRunConfigurationOptions>(project, factory, name) {

    override fun getOptions(): MittsRunConfigurationOptions =
        super.getOptions() as MittsRunConfigurationOptions

    var command: String
        get() = options.command
        set(value) {
            options.command = value
        }

    var workingDirectory: String
        get() = options.workingDirectory
        set(value) {
            options.workingDirectory = value
        }

    /** Where Mitts runs: the configured directory, else the project's. */
    fun effectiveDirectory(): String? = workingDirectory.ifBlank { project.basePath }

    override fun getConfigurationEditor(): SettingsEditor<out RunConfiguration> = MittsSettingsEditor()

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState =
        object : CommandLineState(environment) {
            init {
                consoleBuilder?.addFilter(MittsLocationFilter(project, effectiveDirectory()))
            }

            override fun startProcess(): ProcessHandler {
                val handler = KillableColoredProcessHandler(
                    MittsCommand.commandLine(command, effectiveDirectory()),
                )
                ProcessTerminatedListener.attach(handler)
                return handler
            }
        }
}

class MittsSettingsEditor : SettingsEditor<MittsRunConfiguration>() {
    private val command = JBTextField()
    private val directory = JBTextField()

    override fun resetEditorFrom(configuration: MittsRunConfiguration) {
        command.text = configuration.command
        directory.text = configuration.workingDirectory
    }

    override fun applyEditorTo(configuration: MittsRunConfiguration) {
        configuration.command = command.text
        configuration.workingDirectory = directory.text
    }

    override fun createEditor(): JComponent = FormBuilder.createFormBuilder()
        .addLabeledComponent("Command (after mitts):", command)
        .addLabeledComponent("Working directory (empty: the project's):", directory)
        .panel
}
