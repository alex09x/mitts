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

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service
import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.columns
import com.intellij.ui.dsl.builder.COLUMNS_LARGE
import com.intellij.ui.dsl.builder.panel

/** The Mitts command, for every project (Settings → Tools → Mitts). */
@Service(Service.Level.APP)
@State(name = "MittsSettings", storages = [Storage("mitts.xml")])
class MittsSettings : SimplePersistentStateComponent<MittsSettings.Options>(Options()) {
    class Options : BaseState() {
        var executable by string(DEFAULT)
    }

    var executable: String
        get() = state.executable?.trim()?.takeIf { it.isNotEmpty() } ?: DEFAULT
        set(value) {
            state.executable = value
        }

    companion object {
        const val DEFAULT = "mitts"

        fun getInstance(): MittsSettings = service()
    }
}

class MittsConfigurable : BoundConfigurable("Mitts") {
    override fun createPanel(): DialogPanel = panel {
        val settings = MittsSettings.getInstance()
        row("Mitts executable:") {
            textField()
                .bindText(settings::executable)
                .columns(COLUMNS_LARGE)
                .comment("A path, or a command found on PATH (default: mitts).")
        }
    }
}
