package dev.miku.link.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

object CoucouColors {
    val Bg = Color(0xFF000000)
    val Hero = Color(0xFF0E0F12)
    val Panel = Color(0xFF141518)
    val Panel2 = Color(0xFF1D1F23)
    val Line = Color(0xFF26282D)
    val HeroLine = Color(0x0FFFFFFF)
    val Text = Color(0xFFF5F6F8)
    val TextDim = Color(0xFF9398A1)
    val TextFaint = Color(0xFF6E737C)
    val PrimaryBtn = Color(0xFFF5F6F8)
    val OnPrimaryBtn = Color(0xFF0B0C0E)
    val SecondaryBtn = Color(0xFF292A2D)
    val Online = Color(0xFF34D399)
    val Amber = Color(0xFFF5A524)
    val Danger = Color(0xFFFF8D97)
    val MikuAccent = Color(0xFF39C5BB)
    val AntigravityBlue = Color(0xFF4E9AF7)
}

internal fun DrawScope.drawCoucouWash(color: Color, alpha: Float = 0.45f) {
    drawRect(
        Brush.radialGradient(
            0f to color.copy(alpha = alpha),
            1f to Color.Transparent,
            center = Offset(size.width / 2f, size.height * 1.25f),
            radius = size.width * 0.85f,
        ),
    )
}

@Composable
fun CoucouHeroPanel(
    modifier: Modifier = Modifier,
    wash: Color? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val shape = RoundedCornerShape(bottomStart = 30.dp, bottomEnd = 30.dp)
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(CoucouColors.Hero)
            .border(1.dp, CoucouColors.HeroLine, shape)
            .then(if (wash != null) Modifier.drawBehind { drawCoucouWash(wash, 0.36f) } else Modifier)
            .padding(horizontal = 20.dp, vertical = 18.dp),
        content = content,
    )
}

@Composable
fun CoucouPanel(
    modifier: Modifier = Modifier,
    wash: Color? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val shape = RoundedCornerShape(20.dp)
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(CoucouColors.Panel)
            .border(1.dp, CoucouColors.Line, shape)
            .then(if (wash != null) Modifier.drawBehind { drawCoucouWash(wash, 0.32f) } else Modifier),
        content = content,
    )
}

@Composable
fun CoucouRowDivider() {
    HorizontalDivider(
        modifier = Modifier.padding(horizontal = 16.dp),
        thickness = 1.dp,
        color = CoucouColors.Line,
    )
}

@Composable
fun CoucouPillButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    danger: Boolean = false,
    enabled: Boolean = true,
) {
    val bg = when {
        !enabled -> CoucouColors.Panel2
        primary -> CoucouColors.PrimaryBtn
        danger -> CoucouColors.Danger.copy(alpha = 0.16f)
        else -> CoucouColors.SecondaryBtn
    }
    val fg = when {
        !enabled -> CoucouColors.TextFaint
        primary -> CoucouColors.OnPrimaryBtn
        danger -> CoucouColors.Danger
        else -> CoucouColors.Text
    }
    Box(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(CircleShape)
            .background(bg)
            .then(
                if (danger && enabled) Modifier.border(1.dp, CoucouColors.Danger.copy(alpha = 0.45f), CircleShape)
                else Modifier
            )
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label,
            color = fg,
            fontSize = 14.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
fun CoucouFloatingBottomBar(
    tabs: List<String>,
    selectedIndex: Int,
    onSelect: (Int) -> Unit,
    alertDotIndex: Int? = null,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp, vertical = 10.dp)
            .height(IntrinsicSize.Min)
            .heightIn(min = 64.dp)
            .clip(CircleShape)
            .background(CoucouColors.Panel)
            .border(1.dp, CoucouColors.Line, CircleShape)
            .padding(6.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        tabs.forEachIndexed { idx, label ->
            val on = idx == selectedIndex
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .heightIn(min = 48.dp)
                    .clip(CircleShape)
                    .background(if (on) CoucouColors.Panel2 else Color.Transparent)
                    .selectable(selected = on, role = Role.Tab, onClick = { onSelect(idx) }),
                contentAlignment = Alignment.Center,
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        text = label,
                        color = if (on) CoucouColors.Text else CoucouColors.TextDim,
                        fontSize = 13.sp,
                        fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium,
                        maxLines = 1,
                    )
                    if (alertDotIndex == idx) {
                        Box(
                            Modifier
                                .size(8.dp)
                                .clip(CircleShape)
                                .background(CoucouColors.Amber),
                        )
                    }
                }
            }
        }
    }
}
