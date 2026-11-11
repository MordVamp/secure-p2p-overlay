package org.p2poverlay.android

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Интерфейс на устройстве: вкладки, переходы, настройки режима. */
@RunWith(AndroidJUnit4::class)
class MainScreenTest {
    // на чистой установке запрос разрешения на уведомления перекрыл бы экран
    @get:Rule(order = 0) val perm: GrantPermissionRule =
        if (android.os.Build.VERSION.SDK_INT >= 33) GrantPermissionRule.grant(android.Manifest.permission.POST_NOTIFICATIONS)
        else GrantPermissionRule.grant()

    @get:Rule(order = 1) val rule = createAndroidComposeRule<MainActivity>()

    @Test fun bottomTabsAreShown() {
        listOf("Чаты", "Файлы", "Сеть", "Узел").forEach { rule.onAllNodesWithText(it)[0].assertIsDisplayed() }
    }

    @Test fun nodeTabShowsModesAndScanner() {
        rule.waitUntil(30_000) {
            rule.onAllNodesWithText("Узел")[0].performClick()
            rule.waitForIdle()
            rule.onAllNodesWithText("Сканировать QR", substring = true).fetchSemanticsNodes().isNotEmpty()
        }
        rule.onNodeWithText("Сканировать QR", substring = true).assertIsDisplayed()
        rule.onNodeWithText("Эконом").assertExists()
        rule.onNodeWithText("Максимум").assertExists()
    }

    @OptIn(androidx.compose.ui.test.ExperimentalTestApi::class)
    @Test fun chatsTabHasNewChat() {
        // отладочная сборка на эмуляторе без GPU первые секунды «тормозит» — нажатие повторяем, пока вкладка не откроется
        rule.waitUntil(30_000) {
            rule.onAllNodesWithText("Чаты")[0].performClick()
            rule.waitForIdle()
            // подпись ExtendedFloatingActionButton есть только в необъединённом дереве семантики
            rule.onAllNodesWithText("Новый чат", useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()
        }
        rule.onAllNodesWithText("Новый чат", useUnmergedTree = true)[0].performClick()
        rule.waitUntilAtLeastOneExists(androidx.compose.ui.test.hasText("@псевдоним или NodeID"), 10_000)
        rule.onNodeWithText("@псевдоним или NodeID").assertExists()
    }
}
