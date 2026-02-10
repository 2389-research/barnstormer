// ABOUTME: Initializes SortableJS on board lanes for drag-and-drop card management.
// ABOUTME: Handles reordering within lanes and moving cards between lanes via the API.

(function () {
    'use strict';

    var boardEl = document.getElementById('board');
    if (!boardEl) return;

    var specId = boardEl.dataset.specId;

    // Calculate a midpoint order between two neighbors, defaulting to
    // reasonable bounds when at the edges of a lane.
    function calculateOrder(evt) {
        var items = evt.to.querySelectorAll('.card');
        var newIndex = evt.newIndex;
        var prevOrder = 0;
        var nextOrder = 0;

        if (newIndex > 0) {
            prevOrder = parseFloat(items[newIndex - 1].dataset.order) || 0;
        }

        if (newIndex < items.length - 1) {
            nextOrder = parseFloat(items[newIndex + 1].dataset.order) || (prevOrder + 2);
        } else {
            nextOrder = prevOrder + 2;
        }

        return (prevOrder + nextOrder) / 2;
    }

    document.querySelectorAll('.lane-cards').forEach(function (lane) {
        new Sortable(lane, {
            group: 'cards',
            animation: 150,
            ghostClass: 'sortable-ghost',
            chosenClass: 'sortable-chosen',
            onEnd: function (evt) {
                var cardId = evt.item.dataset.cardId;
                var newLane = evt.to.dataset.lane;
                var newOrder = calculateOrder(evt);

                // Update the data attributes on the moved card
                evt.item.dataset.lane = newLane;
                evt.item.dataset.order = newOrder;

                fetch('/api/specs/' + specId + '/commands', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        type: 'MoveCard',
                        card_id: cardId,
                        lane: newLane,
                        order: newOrder,
                        updated_by: 'human'
                    })
                }).catch(function (err) {
                    console.error('Failed to move card:', err);
                });
            }
        });
    });
})();
