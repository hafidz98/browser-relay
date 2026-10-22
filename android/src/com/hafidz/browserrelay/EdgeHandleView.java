package com.hafidz.browserrelay;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.util.AttributeSet;
import android.view.View;

/**
 * The tools edge handle from the locked mock: a thin pill docked to
 * a screen edge (DOCKED), which pops out as a round button on first
 * tap (ROUND), and becomes the purple x button while the tools
 * sheet is open (OPEN). Purely visual — MainActivity owns the
 * two-tap state machine and positioning.
 */
public class EdgeHandleView extends View {

    public enum State { DOCKED, ROUND, OPEN }
    public enum Side { LEFT, RIGHT }

    private State state = State.DOCKED;
    private Side side = Side.RIGHT;
    private boolean dragging;

    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint edge = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint icon = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path path = new Path();
    private final RectF rect = new RectF();
    /** The colours come from the palette (colors.xml), like the rest. */
    private final int accent;
    private final int panel;
    private final int field;
    private final int border;
    private final int text;

    public EdgeHandleView(Context ctx, AttributeSet attrs) {
        super(ctx, attrs);
        accent = ctx.getColor(R.color.relay_accent);
        panel = ctx.getColor(R.color.relay_panel);
        field = ctx.getColor(R.color.relay_field);
        border = ctx.getColor(R.color.relay_border);
        text = ctx.getColor(R.color.relay_text);
        edge.setStyle(Paint.Style.STROKE);
        edge.setColor(border);
        edge.setStrokeWidth(dp(1));
        icon.setStyle(Paint.Style.STROKE);
        icon.setStrokeCap(Paint.Cap.ROUND);
        icon.setStrokeJoin(Paint.Join.ROUND);
    }

    private float dp(float v) {
        return v * getResources().getDisplayMetrics().density;
    }

    public void setState(State s) {
        state = s;
        invalidate();
    }

    public State getState() {
        return state;
    }

    public void setSide(Side s) {
        side = s;
        invalidate();
    }

    public Side getSide() {
        return side;
    }

    public void setDragging(boolean d) {
        dragging = d;
        invalidate();
    }

    @Override
    protected void onDraw(Canvas c) {
        int w = getWidth();
        int h = getHeight();
        if (w == 0 || h == 0) return;
        float cx = w / 2f;
        float cy = h / 2f;

        if (state == State.DOCKED && !dragging) {
            // Thin pill flush against the edge; only the inner side
            // is rounded (draw a full round rect, then square off the
            // edge side with a plain rect). The hairline keeps it
            // visible against the dark picture (the web client does
            // the same with a 10% white border).
            fill.setColor(panel);
            float r = h / 2f;
            rect.set(0, 0, w, h);
            c.drawRoundRect(rect, r, r, fill);
            if (side == Side.RIGHT) {
                c.drawRect(w / 2f, 0, w, h, fill);
            } else {
                c.drawRect(0, 0, w / 2f, h, fill);
            }
            edge.setColor(border);
            edge.setStrokeWidth(dp(1));
            rect.set(0.5f, 0.5f, w - 0.5f, h - 0.5f);
            c.drawRoundRect(rect, r, r, edge);
            icon.setColor(accent);
            icon.setStrokeWidth(dp(2.6f));
            float s = dp(6.5f);
            path.reset();
            if (side == Side.RIGHT) {
                // chevron pointing left, into the screen
                path.moveTo(cx + s * 0.7f, cy - s);
                path.lineTo(cx - s * 0.7f, cy);
                path.lineTo(cx + s * 0.7f, cy + s);
            } else {
                path.moveTo(cx - s * 0.7f, cy - s);
                path.lineTo(cx + s * 0.7f, cy);
                path.lineTo(cx - s * 0.7f, cy + s);
            }
            c.drawPath(path, icon);
            return;
        }

        if (state == State.OPEN) {
            fill.setColor(accent);
            c.drawCircle(cx, cy, Math.min(w, h) / 2f, fill);
            // white x
            icon.setColor(text);
            icon.setStrokeWidth(dp(2.2f));
            float s = dp(8f);
            c.drawLine(cx - s, cy - s, cx + s, cy + s, icon);
            c.drawLine(cx + s, cy - s, cx - s, cy + s, icon);
            return;
        }

        // ROUND (or dragging): dark circle with the 2x2 grid glyph.
        fill.setColor(field);
        float rad = Math.min(w, h) / 2f;
        c.drawCircle(cx, cy, rad, fill);
        c.drawCircle(cx, cy, rad - dp(0.5f), edge);
        icon.setColor(accent);
        icon.setStrokeWidth(dp(2f));
        float g = dp(7.5f);
        float gap = dp(3.2f);
        float r2 = dp(1.8f);
        rect.set(cx - g - gap, cy - g - gap, cx - gap, cy - gap);
        c.drawRoundRect(rect, r2, r2, icon);
        rect.set(cx + gap, cy - g - gap, cx + g + gap, cy - gap);
        c.drawRoundRect(rect, r2, r2, icon);
        rect.set(cx - g - gap, cy + gap, cx - gap, cy + g + gap);
        c.drawRoundRect(rect, r2, r2, icon);
        rect.set(cx + gap, cy + gap, cx + g + gap, cy + g + gap);
        c.drawRoundRect(rect, r2, r2, icon);
    }
}
