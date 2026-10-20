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

    public EdgeHandleView(Context ctx, AttributeSet attrs) {
        super(ctx, attrs);
        edge.setStyle(Paint.Style.STROKE);
        edge.setColor(0x24FFFFFF);
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
            // edge side with a plain rect).
            fill.setColor(0x8C181818);
            float r = h / 2f;
            rect.set(0, 0, w, h);
            c.drawRoundRect(rect, r, r, fill);
            if (side == Side.RIGHT) {
                c.drawRect(w / 2f, 0, w, h, fill);
            } else {
                c.drawRect(0, 0, w / 2f, h, fill);
            }
            icon.setColor(0xFF8B72F0);
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
            fill.setColor(0xEB8B72F0);
            c.drawCircle(cx, cy, Math.min(w, h) / 2f, fill);
            // white x
            icon.setColor(0xFFFFFFFF);
            icon.setStrokeWidth(dp(2.2f));
            float s = dp(8f);
            c.drawLine(cx - s, cy - s, cx + s, cy + s, icon);
            c.drawLine(cx + s, cy - s, cx - s, cy + s, icon);
            return;
        }

        // ROUND (or dragging): dark circle with the 2x2 grid glyph.
        fill.setColor(0xEB181818);
        float rad = Math.min(w, h) / 2f;
        c.drawCircle(cx, cy, rad, fill);
        c.drawCircle(cx, cy, rad - dp(0.5f), edge);
        icon.setColor(0xFF8B72F0);
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
