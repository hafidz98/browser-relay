package com.hafidz.browserrelay;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.AttributeSet;
import android.view.GestureDetector;
import android.view.MotionEvent;
import android.view.View;

/**
 * Fullscreen view that renders the relay's JPEG frames (fit-center)
 * and translates touch into relay input, mirroring app.js:
 * <ul>
 *   <li>single tap → mouse down+up (left click) at frame coords</li>
 *   <li>one-finger drag → wheel scroll (content follows the finger)</li>
 *   <li>long-press → right click</li>
 * </ul>
 * Coordinate mapping is the inverse of the draw transform, the same
 * math as app.js toFrame(). Every message carries the size of the
 * picture we show (iw/ih), because the PC needs it to put the click
 * exactly where the finger landed.
 */
public class FrameView extends View {

    public interface InputSink {
        boolean canDrive();
        void mouse(String action, float x, float y, String button, int iw, int ih);
        void wheel(float x, float y, float dx, float dy, int iw, int ih);
        /** A touch landed on the content (used to collapse the edge handle). */
        void onContentDown();
    }

    /** One pointer move per this many milliseconds (like app.js). */
    private static final long MOVE_THROTTLE_MS = 30;
    /** A finger step is scaled like the web client does (0.55). */
    private static final float WHEEL_SCALE = 0.55f;
    /** One scroll step is never bigger than this (raw finger pixels). */
    private static final float WHEEL_MAX = 160f;

    private Bitmap bitmap;
    private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final RectF dst = new RectF();
    private float scale = 1f;
    private float offX;
    private float offY;
    private InputSink sink;
    private final GestureDetector gestures;

    // ---- pointer move throttle ----
    private final Handler ui = new Handler(Looper.getMainLooper());
    private long lastMoveAt = 0;
    private float pendingMoveX;
    private float pendingMoveY;
    private boolean hasPendingMove;
    private final Runnable flushMove = new Runnable() {
        @Override
        public void run() {
            if (!hasPendingMove) return;
            hasPendingMove = false;
            lastMoveAt = SystemClock.uptimeMillis();
            if (sink != null && sink.canDrive()) {
                sink.mouse("move", pendingMoveX, pendingMoveY, "left",
                        frameWidth(), frameHeight());
            }
        }
    };

    public FrameView(Context ctx, AttributeSet attrs) {
        super(ctx, attrs);
        setBackgroundColor(Color.BLACK);
        gestures = new GestureDetector(ctx, new GestureDetector.SimpleOnGestureListener() {
            @Override
            public boolean onDown(MotionEvent e) {
                if (sink != null) sink.onContentDown();
                return true;
            }

            @Override
            public boolean onSingleTapUp(MotionEvent e) {
                float[] p = toFrame(e.getX(), e.getY());
                if (p != null && sink != null && sink.canDrive()) {
                    // Move first: the page then knows what is under the
                    // finger (hover) before the click lands.
                    sink.mouse("move", p[0], p[1], "left",
                            frameWidth(), frameHeight());
                    sink.mouse("down", p[0], p[1], "left",
                            frameWidth(), frameHeight());
                    sink.mouse("up", p[0], p[1], "left",
                            frameWidth(), frameHeight());
                }
                return true;
            }

            @Override
            public boolean onScroll(MotionEvent e1, MotionEvent e2,
                                    float distanceX, float distanceY) {
                float[] p = toFrame(e2.getX(), e2.getY());
                if (p != null && sink != null && sink.canDrive()) {
                    // distance is finger travel; wheel deltas move the
                    // content with the finger (same sign convention).
                    sink.wheel(p[0], p[1], wheelDelta(distanceX),
                            wheelDelta(distanceY), frameWidth(), frameHeight());
                    // Keep the remote pointer under the finger so hover
                    // effects work there too.
                    queueMove(p[0], p[1]);
                }
                return true;
            }

            @Override
            public void onLongPress(MotionEvent e) {
                float[] p = toFrame(e.getX(), e.getY());
                if (p != null && sink != null && sink.canDrive()) {
                    sink.mouse("move", p[0], p[1], "right",
                            frameWidth(), frameHeight());
                    sink.mouse("down", p[0], p[1], "right",
                            frameWidth(), frameHeight());
                    sink.mouse("up", p[0], p[1], "right",
                            frameWidth(), frameHeight());
                }
            }
        });
        // Long-press should still fire while a scroll is in progress.
        gestures.setIsLongpressEnabled(true);
    }

    public void setInputSink(InputSink sink) {
        this.sink = sink;
    }

    /** Must be called on the main thread. */
    public void setFrame(Bitmap bmp) {
        Bitmap old = bitmap;
        bitmap = bmp;
        if (old != null && old != bmp) old.recycle();
        invalidate();
    }

    /**
     * View coords → frame pixel coords, or null when there is no
     * frame or the point is outside the drawn image.
     */
    public float[] toFrame(float x, float y) {
        if (bitmap == null || scale <= 0) return null;
        float fx = (x - offX) / scale;
        float fy = (y - offY) / scale;
        if (fx < 0 || fy < 0 || fx > bitmap.getWidth() || fy > bitmap.getHeight()) {
            return null;
        }
        return new float[]{fx, fy};
    }

    /** Width of the picture we show, in pixels (0 when there is none). */
    public int frameWidth() {
        return bitmap != null ? bitmap.getWidth() : 0;
    }

    /** Height of the picture we show, in pixels (0 when there is none). */
    public int frameHeight() {
        return bitmap != null ? bitmap.getHeight() : 0;
    }

    /** One scroll step, scaled and clamped like the web client. */
    private static float wheelDelta(float raw) {
        float v = Math.max(-WHEEL_MAX, Math.min(WHEEL_MAX, raw));
        return v * WHEEL_SCALE;
    }

    /** Send a pointer move, at most one every MOVE_THROTTLE_MS. */
    private void queueMove(float x, float y) {
        pendingMoveX = x;
        pendingMoveY = y;
        hasPendingMove = true;
        long now = SystemClock.uptimeMillis();
        long wait = MOVE_THROTTLE_MS - (now - lastMoveAt);
        if (wait <= 0) {
            ui.removeCallbacks(flushMove);
            flushMove.run();
        } else {
            ui.removeCallbacks(flushMove);
            ui.postDelayed(flushMove, wait);
        }
    }

    @Override
    public boolean onTouchEvent(MotionEvent e) {
        return gestures.onTouchEvent(e);
    }

    @Override
    protected void onDraw(Canvas c) {
        super.onDraw(c);
        if (bitmap == null) return;
        int vw = getWidth();
        int vh = getHeight();
        int bw = bitmap.getWidth();
        int bh = bitmap.getHeight();
        if (vw == 0 || vh == 0 || bw == 0 || bh == 0) return;
        scale = Math.min((float) vw / bw, (float) vh / bh);
        offX = (vw - bw * scale) / 2f;
        offY = (vh - bh * scale) / 2f;
        dst.set(offX, offY, offX + bw * scale, offY + bh * scale);
        c.drawBitmap(bitmap, null, dst, paint);
    }
}
